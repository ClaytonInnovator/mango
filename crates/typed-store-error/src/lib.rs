// Copyright (c) 2021, Facebook, Inc. and its affiliates
// Copyright (c) MangoNet Labs Ltd.
// SPDX-License-Identifier: Apache-2.0
#![warn(
    future_incompatible,
    nonstandard_style,
    rust_2018_idioms,
    rust_2021_compatibility
)]

pub mod errors;

pub use errors::TypedStoreError;
pub type StoreError = errors::TypedStoreError;
