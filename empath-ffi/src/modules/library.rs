use core::fmt::{self, Display};
use std::sync::Arc;

use empath_common::context::Context;
use empath_common::internal;
use empath_tracing::traced;
use libloading::Library;
use serde::Deserialize;

#[derive(Debug)]
pub struct Shared {
    pub name: String,
    pub arguments: Arc<[Arc<str>]>,
    module: Option<super::Mod>,
    library: Option<Library>,
}

#[derive(Deserialize)]
struct SharedConfig {
    name: String,
    #[serde(alias = "args")]
    arguments: Arc<[Arc<str>]>,
}

impl<'de> Deserialize<'de> for Shared {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let config = SharedConfig::deserialize(deserializer)?;
        Ok(Self {
            name: config.name,
            arguments: config.arguments,
            module: None,
            library: None,
        })
    }
}

unsafe impl Send for Shared {}
unsafe impl Sync for Shared {}

impl Display for Shared {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!("{}({:?})", self.name, self.arguments))
    }
}

impl Shared {
    #[traced(instrument(level = tracing::Level::TRACE, skip_all, ret), timing(precision = "us"))]
    pub(super) fn init(&mut self) -> Result<(), super::Error> {
        unsafe {
            let lib = Library::new(&self.name)?;

            let module = lib.get::<super::DeclareModule>(b"declare_module\0")?();
            let response = module.init(&self.arguments);
            internal!("init: {response:#?}");
            self.module = Some(module);
            self.library = Some(lib);

            Ok(())
        }
    }

    #[traced(instrument(level = tracing::Level::TRACE, skip(self, validate_context), ret), timing(precision = "us"))]
    pub(super) fn emit(&self, event: super::Event, validate_context: &mut Context) -> i32 {
        let Some(module) = &self.module else {
            return 0;
        };

        module.emit(event, validate_context)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::Shared;

    #[test]
    fn display_includes_name_and_args() {
        let arguments: Arc<[Arc<str>]> =
            Arc::from(vec![Arc::from("alpha"), Arc::from("beta")].into_boxed_slice());
        let shared = Shared {
            name: "module.so".to_string(),
            arguments,
            module: None,
            library: None,
        };

        let rendered = shared.to_string();

        assert!(rendered.contains("module.so"));
        assert!(rendered.contains("alpha"));
        assert!(rendered.contains("beta"));
    }
}
