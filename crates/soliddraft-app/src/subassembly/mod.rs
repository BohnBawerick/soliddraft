//! Forming a sub-assembly from the open assembly's parts, over MCP. What forming means,
//! what it keeps and what it refuses are `soliddraft-assembly`'s (see
//! `docs/subassemblies.md`); this module holds the command and what it does to the
//! session the window shows: the selection follows the parts into the new file.

pub(crate) mod registry;
pub(crate) mod tools;
