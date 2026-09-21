//! What a hostile guest can make the *host* do while it lifts an answer.
//!
//! Every other limit in [`PluginLimits`] bounds the guest: its instructions
//! (fuel), its wall clock (epochs), its memories, tables and instances (the store
//! limiter). None of them bounds what the host allocates while it copies an answer
//! out of guest memory, and Wasmtime's own default for that budget is 2 GiB - a
//! guest chooses how much memory the host spends. The answer type of a callback is
//! `list<command>`, whose elements carry strings, so the danger is not one large
//! string but a *claim*: a list long enough that the host allocates its own copy
//! of every element, and strings that all point at one region of guest memory so
//! the bytes the host copies are not the bytes the guest stores.
//!
//! [`PluginLimits::hostcall_bytes`] is that bound, and these cases are paired on
//! purpose. Each one runs the *same* guest several times with the bound as the
//! only difference, so the test cannot pass by the guest being unable to make the
//! claim in the first place: with the bound left at Wasmtime's default the claim
//! is transferred and only the batch policy refuses it afterwards, and with the
//! shipped bound the transfer itself is refused.
//!
//! What makes the last claim - that the refusal happens *before* the copy - is
//! Wasmtime's own ordering, not this test: `WasmStr::new` and `WasmList::new`
//! spend the transfer budget from the length the guest declared
//! (`wasmtime-36.0.15/src/runtime/component/func/typed.rs:1604`, `:1859`) and only
//! then does the lift copy (`to_str_from_memory`, `linear_lift_list_from_memory`).
//! What this file proves is the bound itself: it decides admission, at byte
//! magnitude, over the whole answer rather than per string.

use std::convert::Infallible;

use mc_plugin_host::bindings::exports::solaris::plugin::events::{
    Event, EventContext, PlayerJoined,
};
use mc_plugin_host::bindings::exports::solaris::plugin::lifecycle::{
    InitContext, StartupContribution,
};
use mc_plugin_host::{
    CommandBatch, CompiledPlugin, HostError, HostServices, LogLevel, PluginInstance, PluginLimits,
    PluginStartup, engine, linker,
};
use wasm_encoder::reencode::{Error as ReencodeError, Reencode};
use wasm_encoder::{BlockType, Function, Instruction, MemArg, ValType};

mod fixture;

/// Bytes Wasmtime lets a guest hand the host when the host sets no bound of its
/// own. Its documentation calls the default 128 MiB; the constant it applies is
/// `2 << 30`, and either way it is not a bound a server can host third-party code
/// under.
const WASMTIME_DEFAULT_HOSTCALL_FUEL: usize = 2 << 30;

/// The plugin id these instances are bound to.
const PLUGIN: &str = "hostile";

#[derive(Default)]
struct Services {
    id: String,
}

impl HostServices for Services {
    fn log(&mut self, _level: LogLevel, _message: &str) {}

    fn plugin_id(&self) -> &str {
        &self.id
    }
}

/// Limits for one arm: the transfer bound is what a case varies, and the guest is
/// given room to build its claim first.
fn limits(hostcall_bytes: usize, guest_memory_bytes: usize) -> PluginLimits {
    PluginLimits {
        hostcall_bytes,
        guest_memory_bytes,
        ..PluginLimits::default()
    }
}

/// Instantiate the fixture's runtime store with `config` already read.
///
/// The phases run in the two stores both the check and the run path use: the
/// startup phase in a store of its own that is dropped here, then `init` in the
/// store this answers with. The probe in `mode = "configure-isolation"` is what a
/// test uses to tell the two apart from the guest side.
fn guest_with_component(
    bytes: &[u8],
    limits: PluginLimits,
    config: &str,
) -> PluginInstance<Services> {
    let engine = engine(&limits).expect("engine");
    let compiled =
        CompiledPlugin::compile(&engine, bytes, &limits, "0.7.0").expect("the fixture compiles");
    let linker = linker::<Services>(&engine).expect("linker");
    let _ = PluginStartup::instantiate(
        &linker,
        compiled.component(),
        Services {
            id: PLUGIN.to_owned(),
        },
        limits,
    )
    .expect("the startup store instantiates")
    .configure(config)
    .expect("configure");
    let mut plugin = PluginInstance::instantiate(
        &linker,
        compiled.component(),
        Services {
            id: PLUGIN.to_owned(),
        },
        limits,
    )
    .expect("the fixture instantiates");
    plugin
        .init(config, init_context())
        .expect("the fixture's init is harmless");
    plugin
}

fn guest(limits: PluginLimits, config: &str) -> PluginInstance<Services> {
    guest_with_component(&fixture::component_bytes(), limits, config)
}

/// One arm of a startup case: the startup contribution is the answer under test.
fn run_configure(
    limits: PluginLimits,
    config: &str,
) -> Result<Option<StartupContribution>, HostError> {
    let bytes = fixture::component_bytes();
    let engine = engine(&limits).expect("engine");
    let compiled =
        CompiledPlugin::compile(&engine, &bytes, &limits, "0.7.0").expect("the fixture compiles");
    let linker = linker::<Services>(&engine).expect("linker");
    PluginStartup::instantiate(
        &linker,
        compiled.component(),
        Services {
            id: PLUGIN.to_owned(),
        },
        limits,
    )
    .expect("the startup store instantiates")
    .configure(config)
}

fn event_context() -> EventContext {
    EventContext {
        tick: 7,
        first_sequence: 0,
        count: 1,
    }
}

fn join_event() -> Event {
    Event::PlayerJoined(PlayerJoined {
        player: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_owned(),
        session: 3,
        name: "Ada".to_owned(),
    })
}

fn init_context() -> InitContext {
    InitContext {
        plugin_id: PLUGIN.to_owned(),
        api_version: "0.7.0".to_owned(),
        world_fingerprint: String::new(),
    }
}

/// One arm of an event case: the startup phase, `init`, then the answer to one
/// join.
fn run_join(limits: PluginLimits, config: &str) -> Result<CommandBatch, HostError> {
    guest(limits, config).on_events(event_context(), &[join_event()])
}

/// The refusal a claim past the transfer bound must produce.
///
/// The cause is asserted rather than the outcome alone: a refused call says
/// nothing about *why*, and the whole point of the bound is that the reason is the
/// transfer budget rather than the guest's own fuel, memory or a fault. The
/// wording is Wasmtime's, surfaced to the operator unchanged; a dependency bump
/// that renames it must send the reader back to this comment, not be re-pinned
/// blindly. Each case states the same claim under a bound that admits it, so a
/// refusal for any other reason cannot make the case pass.
fn assert_refused_by_the_transfer_budget(answer: Result<(), HostError>) {
    match answer {
        Ok(()) => panic!("the claim must not be admitted"),
        Err(HostError::Trap(message)) => assert!(
            message.contains("hostcalls"),
            "the refusal must name the transfer budget, not another limit: {message}"
        ),
        Err(other) => panic!("the refusal must be the transfer budget, not {other}"),
    }
}

const ON_EVENTS: &str = "solaris:plugin/events@0.7.0#on-events";
const COMMAND_STRIDE: i32 = 120;
const SEND_MESSAGE_TEXT_OFFSET: i32 = 24;
const SEND_MESSAGE_TEXT_LENGTH_OFFSET: i32 = 28;

/// Retarget only the event export of the SDK-built guest after its canonical
/// lowering has materialised the answer. The replacement keeps the first wide
/// string's `(ptr, len)` pair, then makes every later `send-message` point at
/// those same bytes. Its one-byte-shorter lengths make the rewrite observable
/// after host lifting without changing the claimed total enough to evade the
/// transfer budget.
struct AliasWideOutput {
    signature: u32,
    original: u32,
    replacement: u32,
}

impl Reencode for AliasWideOutput {
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
            let index = if export.name == ON_EVENTS {
                self.replacement
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

        let memory = MemArg {
            offset: 0,
            align: 2,
            memory_index: 0,
        };
        let byte_memory = MemArg { align: 0, ..memory };
        // Parameters 0..=4 are the real event export's canonical ABI. Locals
        // hold its result area, list pointer/length, first text pair, and index.
        let mut alias = Function::new([(6, ValType::I32)]);
        for parameter in 0..=4 {
            alias.instruction(&Instruction::LocalGet(parameter));
        }
        alias.instruction(&Instruction::Call(self.original));
        alias.instruction(&Instruction::LocalSet(5));

        // A nonzero result discriminant is the guest's plugin error, which
        // carries no command list to rewrite.
        alias.instruction(&Instruction::LocalGet(5));
        alias.instruction(&Instruction::I32Load8U(byte_memory));
        alias.instruction(&Instruction::I32Eqz);
        alias.instruction(&Instruction::If(BlockType::Empty));
        alias.instruction(&Instruction::LocalGet(5));
        alias.instruction(&Instruction::I32Load(MemArg {
            offset: 4,
            ..memory
        }));
        alias.instruction(&Instruction::LocalSet(6));
        alias.instruction(&Instruction::LocalGet(5));
        alias.instruction(&Instruction::I32Load(MemArg {
            offset: 8,
            ..memory
        }));
        alias.instruction(&Instruction::LocalSet(7));
        alias.instruction(&Instruction::LocalGet(7));
        alias.instruction(&Instruction::I32Const(1));
        alias.instruction(&Instruction::I32GtU);
        alias.instruction(&Instruction::If(BlockType::Empty));
        alias.instruction(&Instruction::LocalGet(6));
        alias.instruction(&Instruction::I32Const(SEND_MESSAGE_TEXT_OFFSET));
        alias.instruction(&Instruction::I32Add);
        alias.instruction(&Instruction::I32Load(memory));
        alias.instruction(&Instruction::LocalSet(8));
        alias.instruction(&Instruction::LocalGet(6));
        alias.instruction(&Instruction::I32Const(SEND_MESSAGE_TEXT_LENGTH_OFFSET));
        alias.instruction(&Instruction::I32Add);
        alias.instruction(&Instruction::I32Load(memory));
        alias.instruction(&Instruction::LocalSet(9));
        alias.instruction(&Instruction::LocalGet(8));
        alias.instruction(&Instruction::I32Const(i32::from(b'x')));
        alias.instruction(&Instruction::I32Store8(byte_memory));
        alias.instruction(&Instruction::I32Const(1));
        alias.instruction(&Instruction::LocalSet(10));
        alias.instruction(&Instruction::Block(BlockType::Empty));
        alias.instruction(&Instruction::Loop(BlockType::Empty));
        alias.instruction(&Instruction::LocalGet(10));
        alias.instruction(&Instruction::LocalGet(7));
        alias.instruction(&Instruction::I32GeU);
        alias.instruction(&Instruction::BrIf(1));
        alias.instruction(&Instruction::LocalGet(6));
        alias.instruction(&Instruction::LocalGet(10));
        alias.instruction(&Instruction::I32Const(COMMAND_STRIDE));
        alias.instruction(&Instruction::I32Mul);
        alias.instruction(&Instruction::I32Add);
        alias.instruction(&Instruction::I32Const(SEND_MESSAGE_TEXT_OFFSET));
        alias.instruction(&Instruction::I32Add);
        alias.instruction(&Instruction::LocalGet(8));
        alias.instruction(&Instruction::I32Store(memory));
        alias.instruction(&Instruction::LocalGet(6));
        alias.instruction(&Instruction::LocalGet(10));
        alias.instruction(&Instruction::I32Const(COMMAND_STRIDE));
        alias.instruction(&Instruction::I32Mul);
        alias.instruction(&Instruction::I32Add);
        alias.instruction(&Instruction::I32Const(SEND_MESSAGE_TEXT_LENGTH_OFFSET));
        alias.instruction(&Instruction::I32Add);
        alias.instruction(&Instruction::LocalGet(9));
        alias.instruction(&Instruction::LocalGet(10));
        alias.instruction(&Instruction::I32Sub);
        alias.instruction(&Instruction::I32Store(memory));
        alias.instruction(&Instruction::LocalGet(10));
        alias.instruction(&Instruction::I32Const(1));
        alias.instruction(&Instruction::I32Add);
        alias.instruction(&Instruction::LocalSet(10));
        alias.instruction(&Instruction::Br(0));
        alias.instruction(&Instruction::End);
        alias.instruction(&Instruction::End);
        alias.instruction(&Instruction::End);
        alias.instruction(&Instruction::End);
        alias.instruction(&Instruction::LocalGet(5));
        alias.instruction(&Instruction::End);
        code.function(&alias);
        Ok(())
    }
}

fn component_with_aliased_wide_output() -> Vec<u8> {
    let _ = fixture::component_bytes();
    let bytes = std::fs::read(
        fixture::repo_root()
            .join("sdk/rust/target/wasm32-unknown-unknown/release/solaris_hello_plugin.wasm"),
    )
    .expect("the SDK-built core module exists");
    let mut imports = 0;
    let mut functions = Vec::new();
    let mut event_export = None;
    for payload in wasmparser::Parser::new(0).parse_all(&bytes) {
        match payload.expect("the SDK-built core module parses") {
            wasmparser::Payload::ImportSection(section) => {
                for import in section.into_imports() {
                    if matches!(
                        import.expect("core import").ty,
                        wasmparser::TypeRef::Func(_) | wasmparser::TypeRef::FuncExact(_)
                    ) {
                        imports += 1;
                    }
                }
            }
            wasmparser::Payload::FunctionSection(section) => {
                functions.extend(
                    section
                        .into_iter()
                        .map(|function| function.expect("core function type")),
                );
            }
            wasmparser::Payload::ExportSection(section) => {
                for export in section {
                    let export = export.expect("core export");
                    if export.name == ON_EVENTS {
                        event_export = Some(export.index);
                    }
                }
            }
            _ => {}
        }
    }
    let original = event_export.expect("SDK guest exports on-events");
    let mut rewrite = AliasWideOutput {
        signature: functions[usize::try_from(original - imports).expect("defined event export")],
        original,
        replacement: imports + u32::try_from(functions.len()).expect("function count"),
    };
    let mut module = wasm_encoder::Module::new();
    rewrite
        .parse_core_module(&mut module, wasmparser::Parser::new(0), &bytes)
        .expect("the SDK-built core module reencodes");
    wit_component::ComponentEncoder::default()
        .module(&module.finish())
        .expect("the rewritten core module carries component metadata")
        .validate(true)
        .encode()
        .expect("the rewritten core module encodes as a component")
}

fn run_aliased_join(limits: PluginLimits, config: &str) -> Result<CommandBatch, HostError> {
    let component = component_with_aliased_wide_output();
    guest_with_component(&component, limits, config).on_events(event_context(), &[join_event()])
}
#[test]
fn one_answer_past_the_transfer_bound_is_never_copied() {
    const CLAIM: usize = 12 * 1024 * 1024;
    let config = format!("mode = \"oversized\"\nsize = {CLAIM}\n");
    // The guest builds the claim and then hands the host a second copy of it
    // while lowering the answer, so it needs room for both.
    const GUEST_MEMORY: usize = 64 * 1024 * 1024;

    // Wasmtime's default: the whole claim is lifted into a `String` of the host's
    // own, and only the batch policy refuses it afterwards. This is the behaviour
    // the bound exists to replace, and it is asserted so that removing the bound
    // cannot leave this file green.
    let copied = run_join(
        limits(WASMTIME_DEFAULT_HOSTCALL_FUEL, GUEST_MEMORY),
        &config,
    );
    assert!(
        matches!(copied, Err(HostError::Answer(_))),
        "with no transfer bound the claim is copied and refused by staging: {copied:?}"
    );

    // The shipped bound, well below the claim: refused during the transfer.
    let bound = PluginLimits::default().hostcall_bytes;
    assert!(
        bound < CLAIM,
        "the case needs a claim past the shipped bound"
    );
    assert_refused_by_the_transfer_budget(run_join(limits(bound, GUEST_MEMORY), &config).map(drop));

    // The same claim, with a bound above it and a batch policy that admits it: the
    // refusal was the transfer bound and nothing else.
    let admitted = run_join(
        PluginLimits {
            hostcall_bytes: CLAIM * 2,
            text_bytes: CLAIM + 1,
            ..limits(bound, GUEST_MEMORY)
        },
        &config,
    )
    .expect("a claim inside every bound must be admitted");
    assert_eq!(admitted.len(), 1, "one command carries the whole claim");
}

#[test]
fn the_transfer_budget_covers_the_whole_answer_not_one_string() {
    // Six strings of 2 MiB: no single string is anywhere near the bound, and
    // their sum is one and a half times it. The shape a list of *aliased* strings
    // would take from the host's side is not built here - a Rust guest cannot make
    // two live strings share one region of its memory, and the receipt records
    // that gap. What this case does show is the property that makes aliasing
    // harmless: the charge is per declared byte over the whole answer, so where a
    // guest's strings point cannot change what the host copies.
    const EACH: usize = 2 * 1024 * 1024;
    const COUNT: usize = 6;
    let config = format!("mode = \"wide\"\nsize = {EACH}\ncount = {COUNT}\n");
    const GUEST_MEMORY: usize = 64 * 1024 * 1024;

    let bound = PluginLimits::default().hostcall_bytes;
    assert!(
        EACH < bound && EACH * COUNT > bound,
        "the case needs one string inside the bound and their sum above it"
    );
    assert_refused_by_the_transfer_budget(run_join(limits(bound, GUEST_MEMORY), &config).map(drop));

    let admitted = run_join(
        PluginLimits {
            hostcall_bytes: EACH * COUNT * 2,
            text_bytes: EACH,
            commands_per_call: COUNT,
            ..limits(bound, GUEST_MEMORY)
        },
        &config,
    )
    .expect("an answer inside every bound must be admitted");
    assert_eq!(admitted.len(), COUNT, "every command of the answer arrives");
    for command in admitted.into_commands() {
        assert!(
            matches!(
                command,
                mc_plugin_host::bindings::solaris::plugin::commands::Command::SendMessage(message)
                    if message.text.len() == EACH
            ),
            "every returned message retains its full payload"
        );
    }
}

#[test]
fn a_nested_startup_contribution_is_held_to_the_same_bound() {
    // Sixteen declarations, each naming the fixture's fixed 64 biomes of 16 KiB:
    // two levels of lists in the startup answer, where the answer is a startup
    // contribution rather than a batch. Few, long names on purpose - a claim of
    // 16 MiB spread over 1024 strings costs the guest far less of its own budget
    // than the same claim over a million, so what refuses it can only be the
    // transfer bound.
    const NAME_BYTES: usize = 16 * 1024;
    const NAMES_PER_DECLARATION: usize = 64;
    const DECLARATIONS: usize = 16;
    let config = format!("mode = \"nested\"\nsize = {NAME_BYTES}\ncount = {DECLARATIONS}\n");
    const GUEST_MEMORY: usize = 64 * 1024 * 1024;

    let bound = PluginLimits::default().hostcall_bytes;
    assert_refused_by_the_transfer_budget(
        run_configure(limits(bound, GUEST_MEMORY), &config).map(drop),
    );

    let contribution = run_configure(
        // Twice the names the answer declares, which covers the list strides as
        // well as the bytes.
        limits(
            NAME_BYTES * NAMES_PER_DECLARATION * DECLARATIONS * 2,
            GUEST_MEMORY,
        ),
        &config,
    )
    .expect("a contribution inside the bound must be admitted")
    .expect("the nested fixture answers a contribution");
    let trees = contribution.trees.expect("the contribution declares trees");
    assert_eq!(trees.len(), DECLARATIONS);
    assert_eq!(trees[0].biomes.len(), NAMES_PER_DECLARATION);
}

#[test]
fn a_guest_cannot_grow_the_stack_past_its_bound() {
    // A frame per call with a live buffer in each: the host's stack bound has to
    // end this call, and end it as a trap the host survives, rather than letting
    // the guest run the thread's stack out. The shallow arm is what keeps the
    // deep one honest - if the fixture's recursion had been flattened into a loop
    // (an optimiser is free to try), ten million iterations would cost far less
    // than one callback's fuel, the deep arm would answer normally, and this test
    // would fail instead of quietly proving nothing about the stack.
    let shallow = "mode = \"recurse\"\ncount = 1000\n";
    let limits = limits(PluginLimits::default().hostcall_bytes, 64 * 1024 * 1024);
    guest(limits, shallow)
        .on_events(event_context(), &[join_event()])
        .expect("a thousand frames fit the stack a guest is given");

    let deep = "mode = \"recurse\"\ncount = 10000000\n";
    let mut plugin = guest(limits, deep);
    let error = plugin
        .on_events(event_context(), &[join_event()])
        .expect_err("unbounded recursion must not answer");
    // `Trap` and not `Budget` is the whole assertion: the host classifies an
    // exhausted fuel budget as `Budget`, so a `Budget` here would mean the guest
    // ran out of instructions rather than out of stack - i.e. that the fixture's
    // recursion had been flattened into a loop (ten million iterations fit a
    // callback's fuel) or that something other than the stack bound stopped it.
    // The shallow arm above fixes that the fixture really recurses.
    assert!(
        matches!(error, HostError::Trap(_)),
        "the stack bound, not a budget, has to stop unbounded recursion: {error}"
    );
    assert!(
        plugin.retired_because().is_some(),
        "the instance is retired like any other trapped guest"
    );
}

#[test]
fn a_trapped_callback_publishes_no_batch() {
    // The guest builds a batch and then faults, which is the plan's unpublished
    // batch: the answer never exists as a value to admit.
    let config = "mode = \"trap\"\ncount = 4\n";
    let limits = limits(PluginLimits::default().hostcall_bytes, 64 * 1024 * 1024);
    let mut plugin = guest(limits, config);

    let error = plugin
        .on_events(event_context(), &[join_event()])
        .expect_err("a guest that faults must not answer with a batch");
    assert!(
        matches!(error, HostError::Trap(_) | HostError::Budget),
        "a fault is reported as a trap, not as an invalid answer: {error}"
    );
    assert!(
        plugin.retired_because().is_some(),
        "the instance is retired, so no later call can produce that batch"
    );
}

#[test]
fn repeated_output_pointers_cannot_reduce_the_transfer_charge() {
    const EACH: usize = 2 * 1024 * 1024;
    const COUNT: usize = 6;
    let config = format!("mode = \"wide\"\nsize = {EACH}\ncount = {COUNT}\n");
    const GUEST_MEMORY: usize = 64 * 1024 * 1024;

    let bound = PluginLimits::default().hostcall_bytes;
    assert!(
        EACH < bound && EACH * COUNT > bound,
        "the aliased answer must exceed the shipped transfer budget"
    );
    assert_refused_by_the_transfer_budget(
        run_aliased_join(limits(bound, GUEST_MEMORY), &config).map(drop),
    );

    let admitted = run_aliased_join(
        PluginLimits {
            hostcall_bytes: EACH * COUNT * 2,
            text_bytes: EACH,
            commands_per_call: COUNT,
            ..limits(bound, GUEST_MEMORY)
        },
        &config,
    )
    .expect("the same aliased answer is lifted when its declared bytes fit");
    assert_eq!(
        admitted.len(),
        COUNT,
        "all aliased commands reached the host"
    );
    for (index, command) in admitted.into_commands().into_iter().enumerate() {
        let mc_plugin_host::bindings::solaris::plugin::commands::Command::SendMessage(message) =
            command
        else {
            panic!("the aliased command must remain a send-message");
        };
        assert_eq!(
            message.text.len(),
            EACH - index,
            "the alias made command {index} declare a distinct length"
        );
        assert_eq!(
            message.text.as_bytes().first(),
            Some(&b'x'),
            "the component rewrote command {index} to the first string's pointer"
        );
        assert!(
            message.text.bytes().skip(1).all(|byte| byte == b'y'),
            "the alias preserves the first string's remaining bytes"
        );
    }
}
