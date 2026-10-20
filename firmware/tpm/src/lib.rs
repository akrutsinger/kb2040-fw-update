//! Async driver for TPM 2.0 chips with the TCG FIFO interface on SPI, such as the Infineon
//! SLB 9670.
//!
//! - [`interface`]: register access, independent of the bus. [`SpiInterface`] implements it.
//! - [`registers`]: register addresses and bits.
//! - [`Tpm`]: the device.
//!
//! The register interface and its SPI framing come from the TCG "PC Client Platform TPM Profile
//! (PTP) Specification", Family 2.0.

#![allow(async_fn_in_trait)]
#![no_std]

pub mod device;
pub mod error;
pub mod interface;
pub mod registers;

pub use device::{Id, Tpm};
pub use error::Error;
pub use interface::{Interface, SpiInterface};
