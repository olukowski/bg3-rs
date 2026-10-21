//! Baldur's Gate 3 materials, without any rendering dependency.
//!
//! A material has two layers (see `docs/research/materials.md`):
//!
//! - a **shader graph** (`Public/**/Assets/Materials/**.lsf`, region `Material`): render-state flags
//!   and the parameters the graph declares, each with a default (`Value`);
//! - a **material instance** (`MaterialBank` resource): `SourceFile` names the graph, and its
//!   parameter list overrides graph defaults. A parameter with `Enabled == true` is overridden
//!   by the instance; otherwise the graph default applies.
//!
//! [`resolve`](resolve::resolve) merges both and classifies the texture parameters into the
//! slots a PBR renderer understands.

pub mod bc;
pub mod convert;
pub mod dds;
pub mod graph;
pub mod params;
pub mod resolve;

pub use graph::{BlendMode, ShaderGraph};
pub use params::{Param, ParamValue, read_params};
pub use resolve::{ResolvedMaterial, Slot, TextureRef, classify, resolve};
