use core::fmt::{self, Display};
use std::sync::Arc;

use libloading::Library;
use serde::{Deserialize, Serialize};

use crate::{internal, smtp::context::Context};

#[allow(
    clippy::unsafe_derive_deserialize,
    reason = "The unsafe aspects have nothing to do with the struct"
)]
#[derive(Serialize, Deserialize)]
pub struct Shared {
    pub name: String,
    pub arguments: Arc<[Arc<str>]>,
    #[serde(skip)]
    module: Option<super::Mod>,
    #[serde(skip)]
    library: Option<Library>,
}

unsafe impl Send for Shared {}
unsafe impl Sync for Shared {}

impl Display for Shared {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!("{}: {:?}", self.name, self.arguments))
    }
}

impl Shared {
    pub(super) fn init(&mut self) -> anyhow::Result<()> {
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

    pub(super) fn emit(&self, event: super::Event, validate_context: &mut Context) -> i32 {
        self.module
            .as_ref()
            .map(|module| module.emit(event, validate_context))
            .unwrap_or_default()
    }
}
