#!/usr/bin/env python3
"""Validate freeswitch-types enums against FreeSWITCH C source.

Replaces the individual check-*.sh scripts with a single tool that:
  - Extracts wire names from C source (enums, string literals, format patterns)
  - Extracts wire names from Rust source (define_header_enum!, Display impls)
  - Compares both directions: missing in Rust, extra in Rust
  - Outputs rust_count/c_count per check (parseable for CI badges)

Usage:
  hooks/check-enums.py [OPTIONS] [CHECK...]

  CHECK is one or more check names: event-types, hangup-causes, channel-states,
  call-states, core-media-vars, conference-vars, sip-header-prefixes,
  event-headers.
  Default: run all checks.

Options:
  --fs-source PATH   FreeSWITCH source tree (default: $FREESWITCH_SOURCE)
  --json             Output JSON (for CI badge consumption)
  --quiet            Print only failing checks

Exit: 0 = all checks pass, 1 = any mismatch.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path

GITHUB_RAW = "https://raw.githubusercontent.com/signalwire/freeswitch/master"


# ── C source extraction helpers ──────────────────────────────────────

HEADER_ADD_RE = re.compile(
    r'switch_event_add_header[^(]*\([^,]+,\s*[^,]+,\s*"([A-Za-z][A-Za-z0-9_-]+)"'
)
PREFIX_SUFFIX_RE = re.compile(r'"%s-([A-Za-z][A-Za-z0-9-]+)"')


def extract_function_body(text: str, func_name: str) -> str:
    """Extract one C function body using brace counting."""
    pattern = re.compile(rf"^[A-Z_a-z].*\b{re.escape(func_name)}\s*\(", re.MULTILINE)
    m = pattern.search(text)
    if not m:
        return ""
    brace = text.find("{", m.start())
    if brace == -1:
        return ""
    depth = 0
    for i in range(brace, len(text)):
        if text[i] == "{":
            depth += 1
        elif text[i] == "}":
            depth -= 1
            if depth == 0:
                return text[brace : i + 1]
    return ""


def extract_headers_from_text(text: str) -> set[str]:
    return set(HEADER_ADD_RE.findall(text))


# ── File resolution ──────────────────────────────────────────────────


class SourceResolver:
    def __init__(self, fs_source: str | None, cache_dir: Path):
        self.fs_source = fs_source
        self.cache_dir = cache_dir

    def resolve(self, rel_path: str) -> Path:
        if self.fs_source:
            p = Path(self.fs_source) / rel_path
            if not p.exists():
                print(f"error: {p} not found", file=sys.stderr)
                sys.exit(1)
            return p
        cached = self.cache_dir / rel_path.replace("/", "_")
        if not cached.exists():
            url = f"{GITHUB_RAW}/{rel_path}"
            print(f"  fetching {rel_path}...", file=sys.stderr)
            self.cache_dir.mkdir(parents=True, exist_ok=True)
            urllib.request.urlretrieve(url, cached)
        return cached

    def read(self, rel_path: str) -> str:
        return self.resolve(rel_path).read_text(errors="replace")


# ── Rust source extraction ───────────────────────────────────────────


def rust_define_header_enum_names(path: Path) -> set[str]:
    """Extract wire names from a define_header_enum! block."""
    text = path.read_text()
    names: set[str] = set()
    in_block = False
    depth = 0
    for line in text.splitlines():
        if "define_header_enum!" in line:
            in_block = True
            depth = 0
        if in_block:
            depth += line.count("{") - line.count("}")
            # error_type: ParseFooError => "message" is macro config, not a variant
            if not line.lstrip().startswith("error_type:"):
                m = re.search(r'=> "([^"]+)"', line)
                if m:
                    names.add(m.group(1))
            if depth <= 0 and names:
                break
    return names


def rust_display_impl_names(
    path: Path, type_name: str, stop_at: str | None = None
) -> set[str]:
    """Extract wire names from a type's Display impl or as_str() match arms.

    Looks, in order, for:
      1. a `wire_enum! { ... pub enum <type_name> { ... } ... }` macro
         invocation (generates as_str/Display/FromStr via a table)
      2. a manual `impl <type_name> { fn as_str(&self) -> &'static str { ... } }`
      3. a manual `impl fmt::Display for <type_name>` block

    Each source is treated equivalently: wire names are collected from
    `=> "NAME"` arrows in the body.
    """
    text = path.read_text()
    names: set[str] = set()

    wire_enum_re = re.compile(
        rf"wire_enum!\s*\{{(?:[^{{}}]|\{{[^{{}}]*\}})*?"
        rf"pub enum {re.escape(type_name)}\s*\{{(.*?)\}}",
        re.DOTALL,
    )
    m = wire_enum_re.search(text)
    if m:
        body = m.group(1)
        for arm in re.finditer(r'=> "([^"]+)"', body):
            if stop_at and arm.group(1) == stop_at:
                break
            names.add(arm.group(1))
        if names:
            return names

    as_str_re = re.compile(
        rf"impl {re.escape(type_name)}\s*\{{.*?"
        rf"fn as_str\(&self\)\s*->\s*&'static str\s*\{{(.*?)\n\s*\}}",
        re.DOTALL,
    )
    m = as_str_re.search(text)
    if m:
        body = m.group(1)
        for arm in re.finditer(r'=> "([^"]+)"', body):
            if stop_at and arm.group(1) == stop_at:
                break
            names.add(arm.group(1))
        if names:
            return names

    pattern = rf"impl fmt::Display for {type_name}"
    in_block = False
    for line in text.splitlines():
        if pattern in line:
            in_block = True
        if in_block:
            if stop_at and f'=> "{stop_at}"' in line:
                break
            m = re.search(r'=> "([^"]+)"', line)
            if m:
                names.add(m.group(1))
            if in_block and line.strip().startswith("}") and names:
                break
    return names


def rust_timetable_suffixes(path: Path) -> set[str]:
    """Extract timetable suffixes from the timetable field definition.

    Handles two source forms:
    - Legacy: ``field!(field_name, "Suffix")`` calls inside from_lookup
    - New: ``field_name => "Suffix"`` rows inside channel_timetable_fields!
    """
    text = path.read_text()
    # Legacy: field! macro calls inside from_lookup
    names = set(re.findall(r'field!\(\w+,\s*"([^"]+)"\)', text))
    if names:
        return names
    # New: declarative channel_timetable_fields! table
    m = re.search(r'channel_timetable_fields!\s*\{(.*?)\}', text, re.DOTALL)
    if m:
        return set(re.findall(r'\w+\s*=>\s*"([^"]+)"', m.group(1)))
    return names


# ── Check definitions ────────────────────────────────────────────────


@dataclass
class CheckResult:
    name: str
    rust_count: int = 0
    c_count: int = 0
    missing: list[str] = field(default_factory=list)
    extra: list[str] = field(default_factory=list)

    @property
    def ok(self) -> bool:
        return not self.missing and not self.extra

    @property
    def n_missing(self) -> int:
        return len(self.missing)

    @property
    def summary(self) -> str:
        if self.n_missing:
            return f"{self.name} {self.rust_count}/{self.c_count} ({self.n_missing} missing)"
        return f"{self.name} {self.rust_count}/{self.c_count}"

    @property
    def badge_message(self) -> str:
        """Badge text: whole number when covered, 'N (M missing)' otherwise."""
        if self.n_missing:
            return f"{self.rust_count} ({self.n_missing} missing)"
        return str(self.rust_count)

    @property
    def badge_color(self) -> str:
        if self.n_missing:
            return "red"
        return "blue"


def check_event_types(repo: Path, src: SourceResolver) -> CheckResult:
    rust_file = repo / "freeswitch-types" / "src" / "event" / "event_type.rs"
    rust = rust_display_impl_names(rust_file, "EslEventType", stop_at="ALL")

    c_text = src.read("libs/esl/src/esl_event.c")
    c_block = re.search(r"static.*EVENT_NAMES\[\].*?{(.*?)};", c_text, re.DOTALL)
    c_names = set(re.findall(r'"([A-Z_]+)"', c_block.group(1))) if c_block else set()
    c_names.discard("ALL")

    return CheckResult(
        name="EslEventType",
        rust_count=len(rust),
        c_count=len(c_names),
        missing=sorted(c_names - rust),
        extra=sorted(rust - c_names),
    )


def check_hangup_causes(repo: Path, src: SourceResolver) -> CheckResult:
    rust_file = repo / "freeswitch-types" / "src" / "channel.rs"
    rust = rust_display_impl_names(rust_file, "HangupCause")

    c_text = src.read("src/include/switch_types.h")
    c_block = re.search(
        r"SWITCH_CAUSE_NONE.*?}\s*switch_call_cause_t;", c_text, re.DOTALL
    )
    c_names = (
        set(re.findall(r"SWITCH_CAUSE_([A-Z_]+)", c_block.group(0)))
        if c_block
        else set()
    )

    return CheckResult(
        name="HangupCause",
        rust_count=len(rust),
        c_count=len(c_names),
        missing=sorted(c_names - rust),
        extra=sorted(rust - c_names),
    )


def check_channel_states(repo: Path, src: SourceResolver) -> CheckResult:
    rust_file = repo / "freeswitch-types" / "src" / "channel.rs"
    rust = rust_display_impl_names(rust_file, "ChannelState")

    c_text = src.read("src/include/switch_types.h")
    c_names = set(re.findall(r"^\t(CS_[A-Z_]+)", c_text, re.MULTILINE))

    return CheckResult(
        name="ChannelState",
        rust_count=len(rust),
        c_count=len(c_names),
        missing=sorted(c_names - rust),
        extra=sorted(rust - c_names),
    )


def check_call_states(repo: Path, src: SourceResolver) -> CheckResult:
    rust_file = repo / "freeswitch-types" / "src" / "channel.rs"
    rust = rust_display_impl_names(rust_file, "CallState")

    c_text = src.read("src/include/switch_types.h")
    c_block = re.search(
        r"CCS_DOWN.*?}\s*switch_channel_callstate_t;", c_text, re.DOTALL
    )
    c_raw = (
        set(re.findall(r"(CCS_[A-Z_]+)", c_block.group(0))) if c_block else set()
    )
    # Rust Display strips the CCS_ prefix
    c_names = {n.removeprefix("CCS_") for n in c_raw}

    return CheckResult(
        name="CallState",
        rust_count=len(rust),
        c_count=len(c_names),
        missing=sorted(c_names - rust),
        extra=sorted(rust - c_names),
    )


def check_core_media_vars(repo: Path, src: SourceResolver) -> CheckResult:
    rust_file = repo / "freeswitch-types" / "src" / "variables" / "core_media.rs"
    rust = rust_define_header_enum_names(rust_file)

    c_text = src.read("src/switch_core_media.c")
    prefixes = set(re.findall(r'set_stats\(session,\s*\S+,\s*"([^"]+)"', c_text))
    body = extract_function_body(c_text, "set_stats")
    suffixes = set(re.findall(r'add_stat(?:_double)?\([^,]+,\s*"([^"]+)"', body))
    c_names = {f"rtp_{p}_{s}" for p in prefixes for s in suffixes}

    return CheckResult(
        name="CoreMediaVariable",
        rust_count=len(rust),
        c_count=len(c_names),
        missing=sorted(c_names - rust),
        extra=sorted(rust - c_names),
    )


CONFERENCE_SOURCES = [
    "mod_conference.c",
    "conference_al.c",
    "conference_api.c",
    "conference_cdr.c",
    "conference_event.c",
    "conference_file.c",
    "conference_loop.c",
    "conference_member.c",
    "conference_record.c",
    "conference_utils.c",
    "conference_video.c",
]

# mod_conference-owned variables whose name carries no "conference" substring.
CONFERENCE_UNPREFIXED = {"supplied_pin", "video_banner_text"}

# conference_utils_combine_flag_var() reads a variable the same way, merging
# its index-addressed writes -- its argument is a variable name too.
CONFERENCE_VAR_RE = re.compile(
    r'(?:switch_channel_(?:set|get)_variable[a-z_]*'
    r'|conference_utils_combine_flag_var)\s*\([^,]+,\s*'
    r'(?:"([A-Za-z_][A-Za-z0-9_]*)"|([A-Z_][A-Z0-9_]*))'
)


def check_conference_vars(repo: Path, src: SourceResolver) -> CheckResult:
    rust_file = repo / "freeswitch-types" / "src" / "variables" / "conference.rs"
    rust = rust_define_header_enum_names(rust_file)

    base = "src/mod/applications/mod_conference/"
    defines = dict(
        re.findall(
            r'#define\s+([A-Z_][A-Z0-9_]*_VARIABLE)\s+"([^"]+)"',
            src.read(base + "mod_conference.h"),
        )
    )

    c_names: set[str] = set()
    for name in CONFERENCE_SOURCES:
        text = src.read(base + name)
        code = "\n".join(
            l for l in text.splitlines() if not l.lstrip().startswith("//")
        )
        for literal, macro in CONFERENCE_VAR_RE.findall(code):
            var = literal or defines.get(macro)
            if var and ("conference" in var or var in CONFERENCE_UNPREFIXED):
                c_names.add(var)

    return CheckResult(
        name="ConferenceVariable",
        rust_count=len(rust),
        c_count=len(c_names),
        missing=sorted(c_names - rust),
        extra=sorted(rust - c_names),
    )


def resolved_dependency_dir(repo: Path, dep: str) -> Path:
    """Source directory of the `dep` crate freeswitch-types resolves to."""
    meta = json.loads(
        subprocess.check_output(
            [
                "cargo",
                "metadata",
                "--format-version",
                "1",
                "--manifest-path",
                str(repo / "Cargo.toml"),
            ],
            text=True,
        )
    )
    packages = {p["id"]: p for p in meta["packages"]}
    types_id = next(
        p["id"] for p in meta["packages"] if p["name"] == "freeswitch-types"
    )
    node = next(n for n in meta["resolve"]["nodes"] if n["id"] == types_id)
    dep_id = next(d["pkg"] for d in node["deps"] if d["name"] == dep)
    return Path(packages[dep_id]["manifest_path"]).parent


def catalog_sip_header_names(repo: Path) -> set[str]:
    """Wire names of SipHeader in the sip-header-catalog freeswitch-types resolves to."""
    lib_rs = resolved_dependency_dir(repo, "sip_header_catalog") / "src" / "lib.rs"
    text = lib_rs.read_text()
    start = text.index("pub enum SipHeader {")
    body = text[start : text.index("\n    }\n", start)]
    return set(re.findall(r'=> "([A-Za-z][A-Za-z0-9-]+)"', body))


def check_sip_header_prefixes(repo: Path, src: SourceResolver) -> CheckResult:
    sip_names = catalog_sip_header_names(repo)
    expected_wire = {f"sip_i_{n.lower().replace('-', '_')}" for n in sip_names}

    c_text = src.read("src/mod/endpoints/mod_sofia/sofia.c")
    c_names = set(re.findall(r'"(sip_i_[a-z_]+)"', c_text))

    covered = c_names & expected_wire
    return CheckResult(
        name="SipHeader",
        rust_count=len(covered),
        c_count=len(c_names),
        missing=sorted(c_names - expected_wire),
    )


def check_event_headers(repo: Path, src: SourceResolver) -> CheckResult:
    """Check EventHeader coverage.

    Extraction sources are classified as:
      - "core": every header from these functions MUST be in EventHeader
        (unless structurally excluded: timetable suffixes).
      - "reference": used only to validate that Rust headers trace back
        to real C code. Not all headers from these sources need to be
        in EventHeader (modules add many context-specific headers).
    """
    rust_file = repo / "freeswitch-types" / "src" / "headers.rs"
    rust = rust_define_header_enum_names(rust_file)
    channel_rs = repo / "freeswitch-types" / "src" / "channel.rs"
    timetable_suffixes = rust_timetable_suffixes(channel_rs)

    canonical = {h.lower(): h for h in rust}

    def normalize(name: str) -> str:
        return canonical.get(name.lower(), name)

    # Two pools: core (bidirectional) and reference (Rust->C only)
    core: dict[str, list[str]] = {}
    reference: dict[str, list[str]] = {}

    def add(pool: dict, names: set[str], source: str):
        for n in names:
            pool.setdefault(normalize(n), []).append(source)

    def func_headers(rel_path: str, func: str) -> set[str]:
        text = src.read(rel_path)
        body = extract_function_body(text, func)
        if not body:
            print(
                f"  warning: {func} not found in {Path(rel_path).name}",
                file=sys.stderr,
            )
            return set()
        return extract_headers_from_text(body)

    # ── Core extractions (bidirectional: C->Rust required) ──

    for func in (
        "switch_event_prep_for_delivery_detailed",
        "switch_event_create_subclass_detailed",
        "switch_event_set_priority",
    ):
        add(core, func_headers("src/switch_event.c", func), f"switch_event.c:{func}()")

    for func in (
        "switch_channel_event_set_basic_data",
        "switch_channel_perform_set_callstate",
    ):
        add(
            core,
            func_headers("src/switch_channel.c", func),
            f"switch_channel.c:{func}()",
        )

    # dequeue_dtmf: need manual regex to avoid matching dequeue_dtmf_string
    c_chan = src.read("src/switch_channel.c")
    m = re.search(
        r"^SWITCH_DECLARE.*switch_channel_dequeue_dtmf\b(?!_string)(.*?)^}",
        c_chan,
        re.MULTILINE | re.DOTALL,
    )
    if m:
        add(
            core,
            extract_headers_from_text(m.group(0)),
            "switch_channel.c:switch_channel_dequeue_dtmf()",
        )

    # Caller profile prefix expansion
    c_caller = src.read("src/switch_caller.c")
    body = extract_function_body(c_caller, "switch_caller_profile_event_set_data")
    if body:
        suffixes = set(PREFIX_SUFFIX_RE.findall(body))
        for suffix in suffixes:
            if suffix in timetable_suffixes:
                continue
            for prefix in ("Caller", "Other-Leg"):
                name = f"{prefix}-{suffix}"
                add(
                    core,
                    {name},
                    f"switch_caller.c:switch_caller_profile_event_set_data() prefix={prefix}",
                )

    # Codec (whole file — only contains codec headers)
    c_codec = src.read("src/switch_core_codec.c")
    add(core, extract_headers_from_text(c_codec), "switch_core_codec.c")

    # Application exec
    c_sess = src.read("src/switch_core_session.c")
    m = re.search(
        r"^SWITCH_DECLARE.*switch_core_session_exec\b(?!u)(.*?)^}",
        c_sess,
        re.MULTILINE | re.DOTALL,
    )
    if m:
        add(
            core,
            extract_headers_from_text(m.group(0)),
            "switch_core_session.c:switch_core_session_exec()",
        )

    # Heartbeat
    add(
        core,
        func_headers("src/switch_core.c", "send_heartbeat"),
        "switch_core.c:send_heartbeat()",
    )

    # Log events
    add(
        core,
        func_headers("src/switch_log.c", "switch_log_meta_vprintf"),
        "switch_log.c:switch_log_meta_vprintf()",
    )

    # ── Reference extractions (Rust->C validation only) ──

    add(
        reference,
        func_headers(
            "src/mod/event_handlers/mod_event_socket/mod_event_socket.c",
            "api_exec",
        ),
        "mod_event_socket.c:api_exec()",
    )

    add(
        reference,
        func_headers(
            "src/mod/endpoints/mod_sofia/sofia.c",
            "sofia_handle_sip_i_notify",
        ),
        "sofia.c:sofia_handle_sip_i_notify()",
    )

    add(
        reference,
        func_headers(
            "src/mod/endpoints/mod_sofia/sofia_reg.c",
            "sofia_reg_fire_custom_gateway_state_event",
        ),
        "sofia_reg.c:sofia_reg_fire_custom_gateway_state_event()",
    )

    add(
        reference,
        func_headers(
            "src/mod/endpoints/mod_sofia/sofia_reg.c",
            "sofia_reg_fire_custom_sip_user_state_event",
        ),
        "sofia_reg.c:sofia_reg_fire_custom_sip_user_state_event()",
    )

    c_xmlrpc = src.read("src/mod/xml_int/mod_xml_rpc/mod_xml_rpc.c")
    add(reference, extract_headers_from_text(c_xmlrpc), "mod_xml_rpc.c")

    # ── Pending-upstream allowlist ──
    # Headers present in Rust but not yet merged in signalwire/master.
    # Each entry must reference an open upstream PR. Remove the entry once
    # the PR merges and the cached download picks it up.
    add(
        reference,
        {"Heartbeat-Interval"},
        "pending: signalwire/freeswitch#3003 (add Heartbeat-Interval to send_heartbeat)",
    )

    # ── Structural exclusions from core ──
    # These are patterns, not individual header names.

    def is_structurally_excluded(name: str) -> bool:
        lower = name.lower()
        # ESL framing headers (not event payload)
        if lower in {
            "content-type", "reply-text", "socket-mode", "event-uuid",
            "control", "command", "action",
        }:
            return True
        # Session exec internal dispatcher fields
        if lower in {
            "execute-app-name", "execute-app-arg", "call-command", "event-lock",
        }:
            return True
        # Switch core non-event internal fields
        if lower in {
            "condition", "domain", "purpose", "old-unique-id",
        }:
            return True
        if lower.startswith("network-"):
            return True
        if lower in {"new-hostname", "old-hostname", "trapped-signal"}:
            return True
        return False

    # ── Compare ──

    # "Missing": in core C but not in Rust (and not structurally excluded)
    missing = sorted(
        h for h in core if h not in rust and not is_structurally_excluded(h)
    )

    # "Extra": in Rust but not in core OR reference (untraced)
    all_c = set(core) | set(reference)
    all_c_lower = {h.lower(): h for h in all_c}
    extra = sorted(h for h in rust if h.lower() not in all_c_lower)

    # c_count = all unique C-traced headers that Rust should cover:
    # core (non-excluded) + reference headers that Rust actually has.
    # Use lowercase for set operations to avoid case mismatches.
    rust_lower = {r.lower() for r in rust}
    c_expected_lower = {h.lower() for h in core if not is_structurally_excluded(h)}
    c_expected_lower |= {h.lower() for h in reference if h.lower() in rust_lower}

    return CheckResult(
        name="EventHeader",
        rust_count=len(rust),
        c_count=len(c_expected_lower),
        missing=missing,
        extra=extra,
    )


# ── Main ─────────────────────────────────────────────────────────────

ALL_CHECKS = {
    "event-types": check_event_types,
    "hangup-causes": check_hangup_causes,
    "channel-states": check_channel_states,
    "call-states": check_call_states,
    "core-media-vars": check_core_media_vars,
    "conference-vars": check_conference_vars,
    "sip-header-prefixes": check_sip_header_prefixes,
    "event-headers": check_event_headers,
}


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "checks",
        nargs="*",
        default=list(ALL_CHECKS.keys()),
        metavar="CHECK",
    )
    parser.add_argument(
        "--fs-source", default=os.environ.get("FREESWITCH_SOURCE")
    )
    parser.add_argument("--json", action="store_true", help="output JSON")
    parser.add_argument("--quiet", action="store_true")
    args = parser.parse_args()

    repo = Path(
        subprocess.check_output(
            ["git", "rev-parse", "--show-toplevel"], text=True
        ).strip()
    )
    cache_dir = repo / ".freeswitch-src"
    resolver = SourceResolver(args.fs_source, cache_dir)

    results: list[CheckResult] = []
    rc = 0

    for name in args.checks:
        if name not in ALL_CHECKS:
            parser.error(
                f"invalid choice: {name!r} (choose from {', '.join(ALL_CHECKS)})"
            )
        result = ALL_CHECKS[name](repo, resolver)
        results.append(result)

        if not result.ok:
            rc = 1

        if args.json or (args.quiet and result.ok):
            continue

        if result.missing:
            print(f"{result.name} missing from Rust:")
            for h in result.missing:
                print(f"  + {h}")
        if result.extra:
            print(f"{result.name} extra in Rust (not traced to C source):")
            for h in result.extra:
                print(f"  - {h}")

        print(result.summary)

    if args.json:
        out = {}
        for r in results:
            out[r.name] = {
                "rust": r.rust_count,
                "c": r.c_count,
                "ok": r.ok,
                "badge_message": r.badge_message,
                "badge_color": r.badge_color,
            }
            if r.missing:
                out[r.name]["missing"] = r.missing
            if r.extra:
                out[r.name]["extra"] = r.extra
        print(json.dumps(out, indent=2))

    return rc


if __name__ == "__main__":
    sys.exit(main())
