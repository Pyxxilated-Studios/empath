use crate::context::ValidationContext;

pub mod module;
pub mod string;

pub type InitFunc = unsafe fn() -> isize;
pub type ValidateData = unsafe fn(&ValidationContext) -> isize;
