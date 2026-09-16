//! Re-export of the shared owner-only file helpers. The implementation lives
//! in `kanna-runtime-defaults` so the task-transfer sidecar writes its own
//! key the same way; see that module for the contract.

pub(crate) use kanna_runtime_defaults::secure_file::atomic_write_0600;
