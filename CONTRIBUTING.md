# Contributing

## Hooks

```sh
./hooks/install.sh   # symlinks the pre-commit and pre-push hooks
```

The pre-commit hook enforces:

- Cargo.lock stays off branches -- it may only be committed on the release tag's own detached commit
- `cargo fmt --check` -- formatting
- `cargo clippy --all-features --all-targets -- -D warnings` -- lint warnings as errors
- `RUSTDOCFLAGS="-D missing_docs" cargo doc` -- all public items documented
- `cargo test --workspace --all-features` -- the full test suite, doctests included, the C oracle properties at 4096 cases unless `PROPTEST_CASES` is set
- `hooks/check-enums.py` -- validates `EslEventType`, `HangupCause`, `ChannelState`, `CallState`, `CoreMediaVariable` (`core-media-vars`), `ConferenceVariable` (`conference-vars`), `SipHeaderPrefix` (`sip-header-prefixes`), and `EventHeader` (`event-headers`) against FreeSWITCH C source
- `hooks/check-source-refs.py` -- verifies every `file.c:NNN` citation against the pinned FreeSWITCH commit

The pre-push hook backstops the Cargo.lock check against a rebase or cherry-pick that reintroduces it after the commit gate ran.

## Testing

Unit and mock-server tests run without external dependencies:

```sh
cargo test --lib
cargo test --test connection_tests --test command_wire_tests \
    --test connection_failure_tests --test reexec_tests
```

Live integration tests read `ESL_HOST` / `ESL_PORT` / `ESL_PASSWORD`, default `localhost:8022`; see [live-test-switch.md](docs/live-test-switch.md) for what the switch must provide. They are `#[ignore]` by default:

```sh
cargo test --test 'live_*' -- --ignored
```

They run in parallel against that one switch and raise its `sessions-per-second` to make that safe. [live-test-switch.md](docs/live-test-switch.md) documents the dialplan, modules, and directory users they expect, and the two rules for writing a new one.

## C oracle

The ports in freeswitch-types are checked against the switch's own C, on every FreeSWITCH tree the crate is used against. The unpublished workspace crate freeswitch-c-oracle compiles that C from each tree hooks/source-refs.yaml names: the commit the source references pin, built as pin, and every entry under trees. Its build script reads each file out of the clone `FREESWITCH_SOURCE` names with git show, extracts what it needs into its build directory and compiles one unit per tree with every symbol prefixed by the tree's name, so one test binary runs each differential property against every built tree. Nothing of the FreeSWITCH tree is committed.

Covered on every tree: the string tokenizer of switch_utils.c, switch_true, switch_url_encode_opt with switch_url_encode, switch_core_url_encode_opt and switch_needs_url_encode, switch_event_create_brackets, switch_channel_expand_variables_check, originate_function from mod_commands.c, and the passes switch_ivr_originate and switch_ivr_enterprise_originate run over a dial string up to each leg's endpoint. Then the hook path an execute_on_* value takes: switch_channel_execute_on_value's split of it, and the argument handling of switch_core_session_exec up to the call into the application. Then what the switch makes of an endpoint's text: inline_dialplan_hunt from mod_dptools.c, switch_channel_str2cause with its CAUSE_CHART, and from mod_sofia.c protect_dest_uri, sofia_contact_function whole and sofia_outgoing_channel from its destination check through the To URI, stopping where it attaches to the profile. From mod_loopback.c, channel_outgoing_channel from the caller profile it clones through the channel name; user_outgoing_channel of mod_dptools.c and group_call_function of mod_commands.c up to their directory lookup.

Stubs stand in for the event, channel, session, profile and gateway calls and report the headers an event holds or reads, variable looked up or set, API called and profile, gateway, registration or host looked up. An event created for channel data, as every event the originate passes install into is, replaces a header by name ignoring case and deletes it on an empty value, the model of `EF_UNIQ_HEADERS` the port's event store is held to. A variable, API, registration or host lookup answers nothing, so a reference expands to an empty string and a bare sofia user is never registered. A profile or gateway is found only when the test names it, with the fixed addresses the Sofia struct documents, and the core's default domain is DEFAULT_DOMAIN. The `<>` event the enterprise originate hands each thread is not modelled, nor the event clone mod_loopback stores. The sofia readers take a destination as mod_sofia receives it; the fork's encoder re-encodes a valid escape, so protect_dest_uri is checked against each tree's own switch_url_encode, and sofia_outgoing_channel agrees across trees once that encoding is done.

The oracle's C lives in freeswitch-c-oracle/c, one unit per area, joined in the order UNITS in build.rs lists them. A line reading `//@ <kind> <path> <argument>` in a unit is replaced by what it names in each tree: each file is parsed with tree-sitter-c and a piece is the whole lines its syntax node spans. define takes a #define by name, function a definition by the name its declarator carries (never its prototype), block the statement opening on a marker line inside a function, through its braces and any chained else, or through its semicolon when it opens none; declaration takes a declaration, typedef or struct definition by the name it declares. A marker must occur once in its function; for a statement the function repeats, block takes `<anchor> => <marker>`, the statement on the first line reading the marker after an anchor that occurs once. No statement of the switch is typed into a unit: its statements are taken by directive, and a unit writes only the harness's loops, labels, locals, stubs and records, its loop headers and locals shaped as the switch's so the taken statements compile in place. Line drift never breaks the extraction, and a tree that renames a function or moves a marker fails the build rather than the oracle; build/extract.rs carries the grammar and its unit tests. To cover another function, write its unit with the directives and stubs it needs, list the unit in UNITS, declare each symbol Rust calls with a `//@ export <symbol> <signature>` line beside it, the Rust parameter list and return from which build.rs generates the Abi field, named for the symbol without its oracle_ or switch_ prefix, and every tree's extern block, expose it on `Oracle`, and write a property through on_every_tree, which hands the property each tree's name so a per-tree expectation can be stated, or against_the_c where every tree must agree. In freeswitch-types a property sits in the c_oracle.rs beside the pass it holds to the C, one per file in src/switch_passes, its seeds under the matching path in proptest-regressions.

To track another tree, add its name, commit and block_parse under trees in hooks/source-refs.yaml, with fetch naming its public remote when it has one; check-source-refs.py carries the entry through --update.

Every tree, the pin under freeswitch included, names the `BlockParse` revision its switch_event_create_brackets runs, in that type's string form. A property whose port renders, parses or reads a block takes the revision from the tree through against_the_c_at_its_revision or tree_block_parse in freeswitch-types' switch_passes, never the default, and every name must parse as a revision the crate has. When a tree's block parse changes, its differential properties fail under the revision it names: append a `BlockParse` variant for the new parse, give it its arm where brackets.rs runs the block's own splits and wherever else the compiler asks, and name that variant under the tree's block_parse.

Each c_oracle.rs module, and src/commands/proptests, compiles only under `--cfg freeswitch_oracle`, which the workspace's .cargo/config.toml sets for every build started inside it. The published freeswitch-types leaves those files out, and its unit tests build and run without the cfg, the oracle crate or either source tree. A `RUSTFLAGS` or `CARGO_ENCODED_RUSTFLAGS` in the environment replaces the config's flags outright, so the pre-commit hook and scripts/ci-test.sh fail when no `c_oracle::` test ran; add `--cfg freeswitch_oracle` to your own flags when you set either.

A property whose expectation differs between trees names the tree it expects, as the URL-encoding rules in freeswitch-c-oracle/tests/url_encode.rs do, and fails on a tree it has no rule for.

Without `FREESWITCH_SOURCE`, or for a tree whose commit the clone lacks, the build prints a cargo warning naming the tree and the reason, and each property prints one skip line for that tree and passes. The pre-commit hook inherits the variable, and its source-reference check refuses a commit without it, so the oracle runs there on every tree the clone holds. CI fetches the pin and every tree with a public remote shallowly and fails when one of those is not built; a tree without one is skipped with a line saying so.

sofia-sip's SDP parser is checked the same way, once rather than per tree, since every tree the crate tracks links the same parser. build.rs reads `sdp_parse.c`, `su_string.c` and the headers they include out of the clone `SOFIA_SIP_SOURCE` names, at the commit hooks/source-refs.yaml pins under `sofia_sip`, and compiles them whole beside a calloc-backed `su_alloc` stub and the configure headers in freeswitch-c-oracle/c/sdp. The shim there calls `sdp_parse` as switch_core_media.c does, flags 0 and the length from `strlen`, and reports the refusal or each section through the callback. In freeswitch-types, sdp/codecs/c_oracle.rs holds `SdpCodecs::parse` to it over generated offers with `against_sofia`. Without the variable the property prints one skip line and passes; the source-reference check refuses a commit without it, as it does without `FREESWITCH_SOURCE`, and CI fetches the pin from its public remote.
