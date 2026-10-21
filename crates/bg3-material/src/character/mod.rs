//! Character shader graphs: which graph is which, their parameters, and the colour math.
//!
//! The math was established from the compiled shaders of the reference build (4.1.1.7631656):
//! the Metal library `Data/Shaders/Metal/Materials.metallib` holds every graph's entry points
//! (`<Module>_<Folder>_<Graph>_ST_DEF_fragmentMain` is the deferred G-buffer writer). Each
//! function is LLVM bitcode (AIR) that `llvm-dis` reads; its metadata names the constant-buffer
//! members (`FloatParameter_<name>`, …) and textures (`Texture2DParameter_<name or ID>`), so the
//! parameters map 1:1 onto the graph's parameter table. The write-up, with every formula's
//! source, is `docs/research/character-shaders.md`. The functions here are fresh Rust
//! implementations of the documented formulas; the viewer's WESL mirrors them, and the unit
//! tests pin both down with synthetic inputs.

pub mod hair;
pub mod kinds;
pub mod params;
pub mod skin;
pub mod vertex_cut;

pub use kinds::{CharKind, UNIFORM_LEN};
pub use params::ParamSet;
pub use skin::SkinKind;
