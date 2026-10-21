//! SolidDraft's assistant, independent of the interface.
//!
//! - [`provider`]: the app-owned [`Provider`] trait, with [`AnthropicProvider`] and
//!   [`OpenRouterProvider`] over the Messages API, streaming server-sent events;
//!   [`MockProvider`] replays recorded streams.
//! - [`choice`]: which provider, key and model to use, from the settings, the environment
//!   and the keys present.
//! - [`conversation`]: the agent loop. It sends the user's text, streams the reply, runs the
//!   tool calls the model makes against a [`ToolHost`] and loops until the model is done.
//! - [`tools`]: tool definitions and the schema check every tool input passes before it runs.
//! - [`mcp`]: the same tools served over the Model Context Protocol, on stdio or HTTP.
//! - [`relay`]: a provider on a server, for the web build, which holds no key.
//! - [`key`]: each provider's API key, from the environment or the OS keychain, never logged.
//!
//! The crate knows nothing about documents or kernels. The app implements [`ToolHost`] over its
//! command layer, so the assistant, an MCP client and the ribbon all run the same commands.

pub mod choice;
pub mod conversation;
pub mod environment;
pub mod key;
pub mod mcp;
pub mod provider;
pub mod relay;
pub mod sse;
pub mod tools;
pub mod wire;

pub use choice::{AssistantSettings, Choice, NoProvider};
pub use conversation::{ChatEvent, Conversation, ConversationConfig};
pub use key::{ApiKey, KeySource};
#[cfg(not(target_arch = "wasm32"))]
pub use provider::{AnthropicProvider, OpenRouterProvider};
pub use provider::{
    Delivery, MockProvider, MockTurn, Provider, ProviderError, ProviderKind, ProviderResponse, StreamEvent,
};
pub use tools::{Caller, ToolHost, ToolOutput, ToolSpec};
pub use wire::{Message, Role};

/// The model the assistant uses on the Anthropic API unless told otherwise.
pub const DEFAULT_MODEL: &str = "claude-opus-5-5";

/// The model the assistant uses on OpenRouter unless told otherwise: the same
/// model as [`DEFAULT_MODEL`], under its id in `GET https://openrouter.ai/api/v1/models`.
pub const OPENROUTER_DEFAULT_MODEL: &str = "anthropic/claude-opus-5.5";
