use std::io::Write;

use super::*;

fn declared(
    index: &str,
) -> Result<Vec<mc_script::ScriptClientViewRequestKind>, LoaderHandshakeError> {
    let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
    archive
        .start_file(
            LOADER_ARTIFACT_INDEX_PATH,
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
    archive.write_all(index.as_bytes()).unwrap();
    let bytes = archive.finish().unwrap().into_inner();
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    let index = read_index_from_artifact_bytes(
        &bytes,
        Path::new("fixture.bundle"),
        bytes.len() as u64,
        &sha256,
    )?;
    declared_screen_kinds(&index, "example")
}

#[test]
fn plugin_opened_screens_do_not_claim_key_requests() {
    let kinds = declared(
        r#"{"schema":2,"screens":[
            {"id":"example:hud","kind":"hud"},
            {"id":"example:construction","kind":"construction"},
            {"id":"example:economy","kind":"economy"},
            {"id":"example:garrison","kind":"garrison"}
        ]}"#,
    )
    .unwrap();
    assert!(kinds.is_empty());
}

#[test]
fn mixed_screens_preserve_unique_key_request_routes() {
    assert_eq!(
        declared(
            r#"{"schema":2,"screens":[
                {"id":"example:overview","kind":"settlement"},
                {"id":"example:hud","kind":"hud"},
                {"id":"example:roster","kind":"settlement"},
                {"id":"example:army","kind":"army"}
            ]}"#,
        )
        .unwrap(),
        vec![
            mc_script::ScriptClientViewRequestKind::Settlement,
            mc_script::ScriptClientViewRequestKind::Army,
        ]
    );
}

#[test]
fn invalid_screen_declarations_still_fail_before_routing() {
    for index in [
        r#"{"schema":2,"screens":[]}"#,
        r#"{"schema":2,"screens":[{"id":"example:unknown","kind":"siege"}]}"#,
        r#"{"schema":2,"screens":[{"id":"other:hud","kind":"hud"}]}"#,
    ] {
        assert!(matches!(
            declared(index),
            Err(LoaderHandshakeError::ArtifactIndex(_))
        ));
    }
}
