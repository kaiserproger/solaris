//! A real guest returns valid effects but traps in canonical post-return.

mod fixture;

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use mc_plugin_host::{DeploymentConfig, DiscoveryMode, HostQueues, PlayerSessions, PluginLimits};
use mc_script::{
    PlayerCommandAdmission, ScriptCommand, ScriptEvent, ScriptPlayerContext, ScriptPlayerId,
};
use wasm_encoder::reencode::{Error as ReencodeError, Reencode};

const CLEANUP: &str = "cabi_post_solaris:plugin/events@0.7.0#on-events";
const PLAYER: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

struct TrapCleanup {
    signature: u32,
    function: u32,
}

impl Reencode for TrapCleanup {
    type Error = Infallible;

    fn parse_function_section(
        &mut self,
        functions: &mut wasm_encoder::FunctionSection,
        section: wasmparser::FunctionSectionReader<'_>,
    ) -> Result<(), ReencodeError<Self::Error>> {
        wasm_encoder::reencode::utils::parse_function_section(self, functions, section)?;
        functions.function(self.signature);
        Ok(())
    }

    fn parse_export_section(
        &mut self,
        exports: &mut wasm_encoder::ExportSection,
        section: wasmparser::ExportSectionReader<'_>,
    ) -> Result<(), ReencodeError<Self::Error>> {
        for export in section {
            let export = export?;
            let index = if export.name == CLEANUP {
                self.function
            } else {
                export.index
            };
            exports.export(export.name, self.export_kind(export.kind)?, index);
        }
        Ok(())
    }

    fn parse_code_section(
        &mut self,
        code: &mut wasm_encoder::CodeSection,
        section: wasmparser::CodeSectionReader<'_>,
    ) -> Result<(), ReencodeError<Self::Error>> {
        for body in section {
            code.raw(body?.as_bytes());
        }
        let mut trap = wasm_encoder::Function::new([]);
        trap.instruction(&wasm_encoder::Instruction::Unreachable);
        trap.instruction(&wasm_encoder::Instruction::End);
        code.function(&trap);
        Ok(())
    }
}

fn component_with_trapping_cleanup() -> Vec<u8> {
    let bytes = std::fs::read(
        fixture::repo_root()
            .join("sdk/rust/target/wasm32-unknown-unknown/release/solaris_hello_plugin.wasm"),
    )
    .unwrap();
    let mut imports = 0;
    let mut functions = Vec::new();
    let mut cleanup = None;
    for payload in wasmparser::Parser::new(0).parse_all(&bytes) {
        match payload.unwrap() {
            wasmparser::Payload::ImportSection(section) => {
                for import in section.into_imports() {
                    if matches!(
                        import.unwrap().ty,
                        wasmparser::TypeRef::Func(_) | wasmparser::TypeRef::FuncExact(_)
                    ) {
                        imports += 1;
                    }
                }
            }
            wasmparser::Payload::FunctionSection(section) => {
                functions.extend(section.into_iter().map(Result::unwrap));
            }
            wasmparser::Payload::ExportSection(section) => {
                for export in section {
                    let export = export.unwrap();
                    if export.name == CLEANUP {
                        cleanup = Some(export.index);
                    }
                }
            }
            _ => {}
        }
    }
    // Append a function and retarget only this export. LLVM may share the
    // original cleanup body with init; changing that body would break startup.
    let mut rewrite = TrapCleanup {
        signature: functions
            [usize::try_from(cleanup.expect("event cleanup export") - imports).unwrap()],
        function: imports + u32::try_from(functions.len()).unwrap(),
    };
    let mut module = wasm_encoder::Module::new();
    rewrite
        .parse_core_module(&mut module, wasmparser::Parser::new(0), &bytes)
        .unwrap();
    wit_component::ComponentEncoder::default()
        .module(&module.finish())
        .unwrap()
        .validate(true)
        .encode()
        .unwrap()
}

struct Sessions;

impl PlayerSessions for Sessions {
    fn session_of(&self, player: &str) -> Option<u64> {
        (player == PLAYER).then_some(7)
    }
}

fn context() -> ScriptPlayerContext {
    ScriptPlayerContext::try_new(PLAYER, "Ada", false, 0.0, 64.0, 0.0).unwrap()
}

#[tokio::test]
async fn cleanup_trap_publishes_nothing_and_another_guest_keeps_its_command() {
    let root = tempfile::tempdir().unwrap();
    let normal = fixture::component_bytes();
    let broken = component_with_trapping_cleanup();
    for (id, command, bytes, config) in [
        ("hello", "hello", normal, "greeting = \"survivor\"\n"),
        (
            "doomed",
            "doomed",
            broken,
            "mode = \"messaging\"\nsize = 1\n",
        ),
    ] {
        let package = root.path().join(id);
        std::fs::create_dir(&package).unwrap();
        std::fs::write(package.join("plugin.toml"), format!(
            "id = \"{id}\"\nname = \"{id}\"\nversion = \"0.1.0\"\napi = \"0.7.0\"\nevents = [\"player.joined\"]\nplayer_commands = [\"{command}\"]\n"
        )).unwrap();
        std::fs::write(package.join("plugin.wasm"), bytes).unwrap();
        std::fs::write(package.join("config.toml"), config).unwrap();
    }
    let limits = PluginLimits::default();
    let packages = mc_plugin_host::discover(
        &DeploymentConfig {
            root: root.path().to_path_buf(),
            mode: DiscoveryMode::Strict,
            expected: vec!["doomed".to_owned(), "hello".to_owned()],
            grants: BTreeMap::new(),
            require_grants: true,
            precommit_hooks: Vec::new(),
        },
        &limits,
    )
    .unwrap()
    .into_packages();
    let host = mc_plugin_host::start_deployment(
        packages,
        limits,
        HostQueues::default(),
        Arc::new(Sessions),
    )
    .unwrap();
    let boundary = host.boundary().clone();
    boundary
        .try_enqueue_event(ScriptEvent::player_joined_with_context(
            ScriptPlayerId::new(7),
            context(),
        ))
        .unwrap();
    assert_eq!(
        boundary.try_enqueue_player_command_with_context(
            ScriptPlayerId::new(7),
            context(),
            "hello"
        ),
        Ok(PlayerCommandAdmission::Enqueued)
    );

    for expected in ["survivor Ada", "Hello from a WASM plugin."] {
        let command = tokio::time::timeout(Duration::from_secs(10), boundary.recv_command())
            .await
            .unwrap()
            .expect("the healthy guest still answers");
        let admitted = boundary.accept_host_command(command).unwrap();
        assert!(
            matches!(admitted.request(), ScriptCommand::SendChatMessage { message, .. } if message == expected)
        );
    }
    // The second callback fences the first event's full host traversal, without
    // assuming which plugin is visited first.
    assert_eq!(boundary.player_command_roots(), ["hello"]);
    boundary.close_event_admission();
    assert!(
        tokio::time::timeout(Duration::from_secs(10), boundary.recv_command())
            .await
            .unwrap()
            .is_none()
    );
    host.stop();
}
