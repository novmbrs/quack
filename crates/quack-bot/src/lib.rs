//! Quack's interaction and gateway event routing.
//!
//! Turns Discord interactions and gateway events into engine calls and turns
//! the results into replies. Every interaction is acknowledged before any
//! database or REST work starts.
