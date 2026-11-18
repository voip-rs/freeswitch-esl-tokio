//! Put a document on an outbound INVITE without it crossing the dial string.
//!
//! A `sip_multipart` value set in the originate's `{...}` block goes through
//! the switch's value tokenizer, which eats apostrophes and reads backslashes.
//! `execute_on_originate` runs an application on the new channel before that
//! channel's session thread starts — before mod_sofia builds the INVITE — so a
//! Lua loader there can read the document from a file and set the variable
//! itself. The block then carries two paths and nothing else.
//!
//! Usage: FREESWITCH_VERSION=1.10.12 cargo run --example originate_multipart_file -- <endpoint> <script> <document>
//!   <endpoint>  a dial string such as sofia/internal/1000@pbx.example.com;
//!               never a loopback, which the hook wedges in CS_INIT
//!   <script>    examples/load_multipart.lua, at the path FreeSWITCH sees it
//!   <document>  the XML to carry, at the path FreeSWITCH sees it
//!   Configure via ESL_HOST, ESL_PORT, ESL_PASSWORD env vars; FREESWITCH_VERSION
//!   (or ESL_BLOCK_PARSE for a dev build) is the parser revision rendered for.
//!   Requires FreeSWITCH with `mod_lua`.
//!
//! Both paths are opened by FreeSWITCH, in its mount namespace and under its
//! uid, so a path that exists here and not there fails on the switch and the
//! INVITE goes out without the part. The `lua` API check below runs the loader
//! through the switch first for exactly that reason.

#[path = "common/block_parse.rs"]
mod block_parse;
mod common;

use freeswitch_esl_tokio::commands::{ExecuteOn, UuidGetVar, UuidKill};
use freeswitch_esl_tokio::variables::SofiaVariable;
use freeswitch_esl_tokio::{
    Application, ChannelVariable, DialString, Endpoint, EslClient, EslResult, MultipartBody,
    Originate, Variables, VariablesType,
};

/// A `-ERR` from `api` is data, not a transport failure, and this example
/// wants to say which command produced it.
async fn api_ok(client: &EslClient, cmd: &str) -> Result<String, Box<dyn std::error::Error>> {
    let resp = client
        .api(cmd)
        .await?;
    let out = resp
        .api_result()
        .map_err(|e| format!("{cmd}: {e}"))?;
    Ok(out.to_string())
}

/// The loader has two modes: run by the hook on a session it sets the
/// variable, run through the `lua` API with no session it reports whether it
/// could read the document. `luarun` would not do: it answers `+OK` before the
/// script runs, whatever happens.
async fn check_loader(client: &EslClient, script: &str, document: &str) -> EslResult<String> {
    let resp = client
        .api(&format!("lua {script} {document}"))
        .await?;
    Ok(resp
        .api_result()?
        .to_string())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args()
        .skip(1)
        .collect();
    let [endpoint, script, document] = args.as_slice() else {
        return Err("usage: originate_multipart_file <endpoint> <script> <document>".into());
    };
    let mut endpoint: Endpoint = endpoint.parse()?;
    // Escaping depends on the switch's parser revision, which the crate never
    // asks the switch for, so the version is stated in the environment.
    let block_parse = block_parse::block_parse_from_env()?;

    let (client, _events) = common::connect_from_env().await?;

    if api_ok(&client, "module_exists mod_lua").await? != "true" {
        return Err("mod_lua is not loaded on the switch".into());
    }
    match check_loader(&client, script, document).await? {
        ok if ok.starts_with("ok ") => println!("loader check: {ok}"),
        other => return Err(format!("loader check failed on the switch: {other}").into()),
    }

    // `ExecuteOn::lua` refuses a path with a space, which mod_lua would split.
    let hook = ExecuteOn::lua(script, [document.as_str()])?;
    let mut vars = Variables::new(VariablesType::Default);
    vars.insert(
        ChannelVariable::ExecuteOnOriginate.as_str(),
        hook.to_string(),
    );
    endpoint.set_variables(Some(vars));
    let originate = Originate::application(endpoint, Application::simple("park"));
    let wire = originate
        .display_with(block_parse)
        .to_string();
    println!("{wire}");

    // `api originate` blocks until the far end answers or refuses.
    let uuid = api_ok(&client, &wire).await?;
    println!("leg {uuid} is up");

    // What the hook set is what mod_sofia put in the INVITE: one `type:body`
    // entry per part, which `MultipartBody` reads back.
    let raw = api_ok(
        &client,
        &UuidGetVar::new(&uuid, SofiaVariable::SipMultipart.as_str()).to_string(),
    )
    .await?;
    match MultipartBody::parse(&raw)? {
        Some(parts) => {
            for item in parts.items() {
                println!(
                    "part {}: {} bytes",
                    item.mime_type,
                    item.data
                        .len()
                );
            }
        }
        None => println!("sip_multipart is not set: the loader did not run, see the switch log"),
    }

    match api_ok(&client, &UuidKill::new(&uuid).to_string()).await {
        Ok(_) => println!("hung up {uuid}"),
        Err(e) => eprintln!("could not hang up {uuid}: {e}"),
    }
    client
        .disconnect()
        .await?;
    Ok(())
}
