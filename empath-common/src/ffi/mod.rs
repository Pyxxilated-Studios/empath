use crate::context::Context;

pub mod module;
pub mod string;

pub type InitFunc = unsafe fn() -> isize;
pub type ValidateData = unsafe fn(&Context) -> isize;
