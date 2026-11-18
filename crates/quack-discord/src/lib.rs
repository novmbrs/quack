//! Quack's Discord adapter, built on Twilight.
//!
//! The gateway, cache, and a rate-limit-aware REST client with typed outcomes
//! (done, refused with a reason, unknown, retry after), command definitions,
//! rendering the neutral message model to Components V2, and decoding
//! interactions. It implements the engine's `Discord` port.
