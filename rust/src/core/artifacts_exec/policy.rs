//! The `[addons]` config block — what the managed-artifact layer may fetch
//! and how a spawned catalog server is contained.
//!
//! [`AddonsConfig`] is the `[addons]` config block. Like `[gateway]`, it is
//! **global-only** (never merged from a project-local `.lean-ctx.toml`), so a
//! cloned, untrusted repo cannot loosen it; an org distributes it via
//! MDM / config-management or pins it through the signed org-policy floor.
//!

use serde::{Deserialize, Serialize};

use super::sandbox::SandboxMode;

/// `[addons]` configuration. Global-only; default is fully permissive so the
/// out-of-the-box experience is unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AddonsConfig {
    /// Refuse every managed fetch: grammar dylibs, the ONNX Runtime dylib,
    /// artifact downloads. The strict egress posture, in one switch.
    pub locked: bool,
    /// Honour a user-override registry (`<data_dir>/addon_registry.json`) only
    /// when it carries a valid signature by a trusted org key.
    pub require_signature: bool,
    /// Sandbox spawned stdio servers without a declared `[capabilities]` block:
    /// `off` | `auto` | `strict` (the legacy global mode).
    pub sandbox: String,
    /// Fail closed when an addon declares restricted `[capabilities]` but no OS
    /// sandbox launcher (sandbox-exec / bwrap) is available to enforce them. Off
    /// by default → best-effort (warn + run) so a missing launcher never blocks
    /// a spawn; orgs that require real enforcement set this to `true`.
    pub enforce_capabilities: bool,
    /// Zero-config grammar-addon fetch (#690): transparently download a
    /// SHA-256-pinned grammar dylib on first use of a registry-covered file
    /// extension. On by default (a grammar addon is a parsing fallback, not a
    /// spawned server). Orgs with a strict egress/DLP posture set this to
    /// `false` — reads then degrade to the regex-signature fallback, exactly
    /// like offline. `policy = locked` implies the same.
    pub grammar_auto_fetch: bool,
}

impl Default for AddonsConfig {
    fn default() -> Self {
        Self {
            locked: false,
            require_signature: false,
            sandbox: SandboxMode::Off.as_str().to_string(),
            enforce_capabilities: false,
            grammar_auto_fetch: true,
        }
    }
}

impl AddonsConfig {
    /// The parsed sandbox mode.
    #[must_use]
    pub fn sandbox_mode(&self) -> SandboxMode {
        SandboxMode::parse(&self.sandbox)
    }
}
