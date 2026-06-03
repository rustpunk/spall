//! Embedding `spall-openapi`: load a spec, build a neutral request, run it
//! through *your own* transport, de-paginate the response into a flat item
//! stream, and assemble an OAuth2 client-credentials token request — all without
//! an HTTP client in this crate.
//!
//! `spall-openapi` ships no HTTP client on purpose — it is the transport-neutral
//! contract, not a client. So this example plugs in a tiny in-memory stub
//! transport (canned JSON pages) to show the full contract end to end without
//! touching the network. Swap [`stub_fetch`] for a real `reqwest`/`ureq` call
//! that turns an [`HttpRequestSpec`](spall_openapi::HttpRequestSpec) into a
//! [`ResponseStream`] and the rest of this file is unchanged.
//!
//! Run with: `cargo run -p spall-openapi --example embed`

use std::io::Cursor;

use indexmap::IndexMap;
use secrecy::SecretString;
use spall_core::value::SpallValue;
use spall_openapi::{
    DataPath, Headers, ItemStream, Paginator, RequestBody, ResponseStream, Status, StreamError,
    build_request, oauth2_client_credentials_request,
};

/// A minimal spec whose `listWidgets` operation returns a top-level JSON array —
/// exactly the shape [`DataPath::TopLevel`] de-paginates.
const SPEC: &str = r#"{
  "openapi": "3.0.0",
  "info": { "title": "Widget API", "version": "1.0.0" },
  "servers": [{ "url": "https://api.example.com" }],
  "paths": {
    "/widgets": {
      "get": {
        "operationId": "listWidgets",
        "parameters": [
          { "name": "limit", "in": "query", "required": false,
            "schema": { "type": "integer" } }
        ],
        "responses": { "200": { "description": "a page of widgets" } }
      }
    }
  }
}"#;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Load the spec into spall's resolved IR (`$ref` resolution, parameter
    //    merging, and lenient parsing all happen here).
    let spec = spall_core::loader::load_spec_from_bytes(SPEC.as_bytes(), "embed-example")?;
    // spall normalizes operationIds to lowercase (the form the CLI exposes as a
    // subcommand), so the declared `listWidgets` resolves to `listwidgets`.
    let op = spec
        .operations
        .iter()
        .find(|o| o.operation_id == "listwidgets")
        .expect("the embedded spec declares listWidgets");

    // 2. Build a transport-neutral request: operation + typed args ->
    //    HttpRequestSpec. No I/O, no HTTP client, no auth (auth contributors are
    //    a separate, opt-in step).
    let mut args = IndexMap::new();
    args.insert("limit".to_string(), SpallValue::U64(2));
    let req = build_request(op, &spec, None, &args, None, &[])?;
    println!("{} {}", req.method, req.url);
    println!("query: {:?}", req.query);

    // 3. The caller executes. A real embedder sends `req` with its own HTTP
    //    client and wraps the response in a `ResponseStream`; here a stub
    //    transport returns canned pages so the example needs no network.
    let first = stub_fetch(&req.url)?;

    // 4. De-paginate. `ItemStream` follows the `Link: rel=next` header across
    //    pages and yields a flat stream of items, fetching the next page only
    //    when the current one drains — no page is buffered whole.
    let items = ItemStream::paginated(
        first,
        DataPath::TopLevel,
        Paginator::default(),
        Box::new(|next_url: &str| stub_fetch(next_url)),
    );

    println!("--- widgets (across all pages) ---");
    for item in items {
        // Stream faults (a broken body, a failing fetch, an oversized item)
        // surface inline as `Err(StreamError)`; `?` ends the loop on the first.
        let widget = item?;
        println!("{widget}");
    }

    // 5. The auth contract is neutral too. spall-openapi builds OAuth2 token
    //    requests as plain HttpRequestSpecs — an embedder running the
    //    client-credentials grant builds the token request here, sends it through
    //    the *same* transport used above, and feeds the returned access token back
    //    as a Bearer contributor on subsequent calls. No HTTP client is needed in
    //    this crate to assemble it.
    let client_secret = SecretString::new("s3cr3t".into());
    let token_req = oauth2_client_credentials_request(
        "https://auth.example.com/oauth/token",
        "widget-cli",
        &client_secret,
        Some("widgets:read"),
    );
    println!("--- client-credentials token request ---");
    println!("{} {}", token_req.method, token_req.url);
    if let Some(RequestBody::Form(fields)) = &token_req.body {
        // grant_type / client_id / client_secret / scope — urlencoded by the transport.
        let names: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
        println!("form fields: {names:?}");
    }

    Ok(())
}

/// A stand-in transport: maps a URL to a canned [`ResponseStream`]. Page 1 links
/// to page 2 via an RFC 5988 `Link` header; page 2 has no `next`, so the chain
/// ends. This single function is the seam a real embedder fills with an HTTP
/// client — everything in `main` stays the same.
fn stub_fetch(url: &str) -> Result<ResponseStream, StreamError> {
    let (body, next): (&str, Option<&str>) = if url.contains("page=2") {
        (r#"[{"id":3,"name":"gamma"}]"#, None)
    } else {
        (
            r#"[{"id":1,"name":"alpha"},{"id":2,"name":"beta"}]"#,
            Some("https://api.example.com/widgets?page=2"),
        )
    };

    // Header names are lowercased per the `Headers` contract; `Paginator` reads
    // the `link` header and follows the `rel="next"` target.
    let mut headers = Headers::new();
    headers.insert("content-type".to_string(), "application/json".to_string());
    if let Some(next_url) = next {
        headers.insert("link".to_string(), format!("<{next_url}>; rel=\"next\""));
    }

    Ok(ResponseStream {
        status: Status::from(200),
        headers,
        // A real transport hands back a streaming body; an in-memory `Cursor`
        // is the simplest `Read + Send` that satisfies the same contract.
        body: Box::new(Cursor::new(body.as_bytes().to_vec())),
    })
}
