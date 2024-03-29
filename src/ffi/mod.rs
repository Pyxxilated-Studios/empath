use crate::smtp::context::Context;

pub mod modules;
pub mod string;

#[expect(dead_code)]
pub type InitFunc = unsafe fn() -> isize;
#[expect(dead_code)]
pub type ValidateData = unsafe fn(&Context) -> isize;
