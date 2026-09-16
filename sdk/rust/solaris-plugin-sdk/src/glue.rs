//! Glue between [`crate::Plugin`] and the generated guest exports.
//!
//! The contract's exported functions carry no receiver: a component's functions
//! are instance state, not object methods. The SDK therefore owns the one plugin
//! value of the instance in a thread-local cell - a component instance is
//! single-threaded here, and the host never calls two of its callbacks at once -
//! and forwards every exported call into it through [`crate::Plugin`].

/// Export one [`crate::Plugin`] implementation as this component's guest.
///
/// The type must implement [`Default`]: the host creates the instance once, and a
/// plugin keeps whatever state it needs inside itself.
#[macro_export]
macro_rules! export_plugin {
    ($plugin:ty) => {
        const _: () = {
            use $crate::{events as __events, lifecycle as __lifecycle};
            use std::cell::RefCell;

            /// The marker type the generated ABI exports stand for.
            struct SolarisPlugin;

            thread_local! {
                static PLUGIN: RefCell<Option<$plugin>> = const { RefCell::new(None) };
            }

            /// Run `body` against this instance's plugin value, creating it on
            /// the first callback.
            fn with_plugin<R>(body: impl FnOnce(&mut $plugin) -> R) -> R {
                PLUGIN.with(|cell| {
                    let mut slot = cell.borrow_mut();
                    let plugin = slot.get_or_insert_with(<$plugin>::default);
                    body(plugin)
                })
            }

            impl __lifecycle::Guest for SolarisPlugin {
                fn configure(
                    config: String,
                ) -> Result<Option<$crate::RulePlan>, $crate::PluginError> {
                    let config = $crate::Config::new(&config);
                    with_plugin(|plugin| {
                        <$plugin as $crate::Plugin>::configure(plugin, &config).map_err(Into::into)
                    })
                }

                fn init(
                    config: String,
                    context: $crate::InitContext,
                ) -> Result<Vec<$crate::Command>, $crate::PluginError> {
                    let config = $crate::Config::new(&config);
                    with_plugin(|plugin| {
                        <$plugin as $crate::Plugin>::init(plugin, &config, &context)
                            .map_err(Into::into)
                    })
                }

                fn shutdown() -> Result<(), $crate::PluginError> {
                    with_plugin(|plugin| {
                        <$plugin as $crate::Plugin>::shutdown(plugin).map_err(Into::into)
                    })
                }
            }

            impl __events::Guest for SolarisPlugin {
                fn on_events(
                    context: $crate::EventContext,
                    events: Vec<$crate::Event>,
                ) -> Result<Vec<$crate::Command>, $crate::PluginError> {
                    with_plugin(|plugin| {
                        <$plugin as $crate::Plugin>::on_events(plugin, &context, &events)
                            .map_err(Into::into)
                    })
                }
            }

            $crate::__export!(SolarisPlugin);
        };
    };
}
