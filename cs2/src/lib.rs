#![feature(pointer_byte_offsets)]
#![feature(maybe_uninit_uninit_array)]
#![feature(const_trait_impl)]
#![feature(iterator_try_collect)]

mod handle;
pub use handle::*;

mod entity;
pub use entity::*;

mod offsets;
pub use offsets::*;

pub mod offsets_manual;

mod schema;
pub use schema::*;

mod model;
pub use model::*;

mod ptr;
pub use ptr::*;

mod globals;
pub use globals::*;

mod signature;
pub use signature::*;

mod build;
pub use build::*;
