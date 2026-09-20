#[path = "tests/support.rs"]
mod support;
pub(crate) use support::save_all_test_config;
#[path = "tests/admission.rs"]
mod admission;
#[path = "tests/checkpoint.rs"]
mod checkpoint;
#[path = "tests/lifecycle.rs"]
mod lifecycle;
#[path = "tests/physics.rs"]
mod physics;
