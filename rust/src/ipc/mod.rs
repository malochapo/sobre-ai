//! Process helpers: spawn detached, check, signal and find processes. No
//! socket or pipe: sobre's processes talk over stdio (MCP) and loopback HTTP
//! (the proxy).

pub mod process;
