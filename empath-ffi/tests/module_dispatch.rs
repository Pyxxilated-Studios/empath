use std::time::Duration;

use empath_common::context::Context;
use empath_common::status::Status;
use empath_ffi::modules::test::TestModule;
use empath_ffi::modules::{
    DispatchErrorKind, DispatchFailureMode, DispatchResult, Ev, Event, ModuleDispatchConfig,
    ModuleIsolation, ModuleRegistry, ModuleRegistryBuilder, validate,
};

fn build_registry(config: ModuleDispatchConfig) -> Result<ModuleRegistry, String> {
    ModuleRegistryBuilder::new()
        .with_dispatch_config(config)
        .build()
        .map_err(|err| format!("module registry builds: {err}"))
}

fn configure_test_module(
    registry: &ModuleRegistry,
    sleep_millis: Option<u64>,
    return_code: i32,
) -> Result<(), String> {
    let mut found = false;
    for module in registry.modules() {
        if let Some(TestModule(lock)) = module.as_any().downcast_ref() {
            found = true;
            let mut inner = lock
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            inner.sleep_millis = sleep_millis;
            inner.return_code = return_code;
        }
    }
    if found {
        Ok(())
    } else {
        Err("test module missing from registry".to_string())
    }
}

#[tokio::test]
#[cfg_attr(
    miri,
    ignore = "Miri does not support kqueue-backed Tokio runtime on macOS"
)]
async fn dispatch_event_timeout_fail_closed_returns_error() -> Result<(), String> {
    let config = ModuleDispatchConfig {
        event_timeout_secs: Some(0),
        event_failure_mode: DispatchFailureMode::FailClosed,
        isolation: ModuleIsolation::SpawnBlocking,
        ..ModuleDispatchConfig::default()
    };
    let registry = build_registry(config)?;
    configure_test_module(&registry, Some(5), 0)?;

    let mut context = Context::default();
    let result = registry
        .dispatch(Event::Event(Ev::ConnectionOpened), &mut context)
        .await;

    let DispatchResult::Error {
        kind: DispatchErrorKind::Timeout { timeout },
        ..
    } = result
    else {
        return Err("expected timeout error".to_string());
    };
    if timeout != Duration::from_secs(0) {
        return Err(format!("unexpected timeout {timeout:?}"));
    }

    Ok(())
}

#[tokio::test]
#[cfg_attr(
    miri,
    ignore = "Miri does not support kqueue-backed Tokio runtime on macOS"
)]
async fn dispatch_validation_timeout_fail_closed_rejects() -> Result<(), String> {
    let config = ModuleDispatchConfig {
        validation_timeout_secs: Some(0),
        validation_failure_mode: DispatchFailureMode::FailClosed,
        isolation: ModuleIsolation::SpawnBlocking,
        ..ModuleDispatchConfig::default()
    };
    let registry = build_registry(config)?;
    configure_test_module(&registry, Some(5), 0)?;

    let mut context = Context::default();
    let result = registry
        .dispatch(Event::Validate(validate::Event::Connect), &mut context)
        .await;

    let DispatchResult::Reject { status, .. } = result else {
        return Err("expected rejection".to_string());
    };
    if status != Status::ActionUnavailable {
        return Err(format!("unexpected status {status:?}"));
    }

    Ok(())
}
