//! History compaction.
//!
//! Manages conversation history size by summarizing older messages
//! when the context window limit approaches. Supports multiple
//! compaction strategies:
//!
//! - Auto-compact: triggered when token count exceeds threshold
//! - Reactive compact: triggered by API context overflow errors
//! - Microcompact: compresses individual tool results

// Future: compaction strategies, token budget management
