use crate::smtp::context::Context;

pub mod modules;
pub mod string;

#[allow(dead_code)]
pub type InitFunc = unsafe fn() -> isize;
#[allow(dead_code)]
pub type ValidateData = unsafe fn(&Context) -> isize;
