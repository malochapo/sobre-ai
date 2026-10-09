use thiserror::Error;

#[derive(Error, Debug)]
pub enum SobreError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Config(#[from] ConfigError),

    #[error("parse error: {0}")]
    Parse(String),
}

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("Cannot determine config path")]
    MissingPath,

    #[error("Unknown config key: {key}")]
    UnknownKey { key: String },

    #[error("Cannot read config: {source}")]
    Read {
        #[source]
        source: std::io::Error,
    },

    #[error("Config parse error: {source}")]
    ParseToml {
        #[source]
        source: toml::de::Error,
    },

    #[error("Invalid value for '{key}': {message}")]
    InvalidValue { key: String, message: String },

    #[error("Error saving config: {source}")]
    Save {
        #[source]
        source: Box<SobreError>,
    },

    #[error("Expected bool (true/false), got: {value}")]
    ExpectedBool { value: String },

    #[error("Expected integer, got: {value}")]
    ExpectedInteger { value: String },

    #[error("Expected unsigned integer, got: {value}")]
    ExpectedUnsignedInteger { value: String },

    #[error("Expected number, got: {value}")]
    ExpectedNumber { value: String },

    #[error("Invalid value '{value}'. Allowed: {allowed}")]
    InvalidEnumValue { value: String, allowed: String },

    #[error("Cannot set table '{value}' via CLI. Edit config.toml directly.")]
    CannotSetTable { value: String },

    #[error(
        "Cannot set '{key}': '{part}' already holds a non-table value in config.toml. Fix or remove that key first."
    )]
    NonTableParent { key: String, part: String },

    #[error("{0}")]
    Message(String),
}

#[derive(Error, Debug)]
pub enum DispatchError {
    #[error("path resolution failed: {message}")]
    PathResolution { message: String },

    #[error("{message}")]
    Tool { message: String },
}

impl From<String> for ConfigError {
    fn from(message: String) -> Self {
        ConfigError::Message(message)
    }
}

impl From<&str> for ConfigError {
    fn from(message: &str) -> Self {
        ConfigError::Message(message.to_string())
    }
}

impl From<toml::de::Error> for SobreError {
    fn from(e: toml::de::Error) -> Self {
        SobreError::Config(ConfigError::Message(e.to_string()))
    }
}

impl From<serde_json::Error> for SobreError {
    fn from(e: serde_json::Error) -> Self {
        SobreError::Parse(e.to_string())
    }
}
