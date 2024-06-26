// Copyright (c) MangoNet Labs Ltd.
// SPDX-License-Identifier: Apache-2.0

mod builder;
pub mod interface;

// TODO remove the pub(crater) once indexer_v2.rs is renamed to lib.rs
pub(crate) mod fetcher;
pub(crate) mod runner;

pub use builder::IndexerBuilder;
pub use interface::Handler;
