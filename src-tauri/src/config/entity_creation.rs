//! Parent module for creating AC entities on disk.
//! Declares the crate-private submodules: copies, directories, matrices, names, provenance,
//! roles and teams. This file holds no logic or state; all of it lives in those submodules.

pub(crate) mod copies;
pub(crate) mod directories;
pub(crate) mod matrices;
pub(crate) mod names;
pub(crate) mod provenance;
pub(crate) mod roles;
pub(crate) mod teams;
