//! This module contains all code relevant to Voice within Waz.
//!
//! Voice is used for voice input within Waz.

// Waz Wave 6-1: `pub(crate) mod transcribe` is physically deleted along with `ServerApi::transcribe`.
// Atomic module `transcribe/api/{request,response}` only for deleted cloud `/ai/transcribe` endpoint
// The wire type. Local speech uses `voice/transcribe.rs::Transcribe` trait + `TranscribeError`.
