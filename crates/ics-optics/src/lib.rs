//! IC Studio image sensor optics: optical constants of pixel materials, thin-film optics and a
//! ray-traced pixel stack model giving quantum efficiency, optical crosstalk and photocurrent.
//!
//! The pixel model is first order: geometric rays through the microlens and the thick layers,
//! coherent thin-film optics for the anti-reflection stack on silicon, Beer-Lambert absorption
//! in silicon collected over the photodiode depth. Diffraction, which matters once the pitch
//! nears the wavelength, is not modelled.

// Index-based loops read more naturally in the numerical kernels.
#![allow(clippy::needless_range_loop)]

pub mod calibrate;
pub mod carriers;
pub mod device;
pub mod light;
pub mod material;
// Measured values that happen to be close to mathematical constants.
#[allow(clippy::approx_constant)]
mod nk_data;
pub mod pixel;
pub mod rcwa;
pub mod tmm;

pub use material::{Material, Materials};
pub use pixel::{PixelResult, PixelStack, simulate};
