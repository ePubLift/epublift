// epublift - Optimize EPUB files: convert images to WebP and upgrade to EPUB 3.3.
// Copyright (C) 2024  Baris Kayadelen
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! [EXPERIMENTAL] "Smart Import": AI OCR (PDF → Markdown) feeding the offline
//! Markdown core.
//!
//! This is the **only** path in epublift that sends user content to a third party,
//! so it is deliberately quarantined: opt-in `smart-import` build feature,
//! bring-your-own API key, and isolated from the local [`crate::markdown`] /
//! [`crate::pdf`] importers. The provider returns Markdown (plus images); we
//! materialise that to a temp dir and hand it to the offline Markdown → EPUB
//! engine — so everything after the OCR call is the same local code path.

use std::path::Path;

use anyhow::{Context, Result, bail};

mod mistral;

/// An AI OCR provider. Today only Mistral; the enum and `/config` plumbing are
/// built so more (Claude, GPT, …) can be added without UI rework.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Mistral,
}

impl Provider {
    /// Stable id used on the wire (UI dropdown value, form field).
    pub fn id(self) -> &'static str {
        match self {
            Provider::Mistral => "mistral",
        }
    }

    /// Human label for the UI.
    pub fn label(self) -> &'static str {
        match self {
            Provider::Mistral => "Mistral OCR",
        }
    }

    /// Environment variable the server reads the API key from. The key stays
    /// server-side and is NEVER sent to the browser.
    pub fn env_var(self) -> &'static str {
        match self {
            Provider::Mistral => "MISTRAL_API_KEY",
        }
    }

    pub fn from_id(s: &str) -> Option<Self> {
        match s {
            "mistral" => Some(Provider::Mistral),
            _ => None,
        }
    }

    /// All known providers, in display order.
    pub fn all() -> &'static [Provider] {
        &[Provider::Mistral]
    }
}

/// Options for a Smart Import.
#[derive(Debug, Clone)]
pub struct SmartOptions {
    pub provider: Provider,
    /// The provider API key (read server-side from [`Provider::env_var`]).
    pub api_key: String,
    /// Content language (BCP-47) passed through to the Markdown importer.
    pub language: Option<String>,
}

/// OCR `pdf` with the chosen provider, materialise the Markdown + images under
/// `workdir`, and produce a reflow EPUB at `output`.
pub fn import(
    pdf: &[u8],
    output: &Path,
    workdir: &Path,
    opts: &SmartOptions,
) -> Result<crate::markdown::ImportSummary> {
    if opts.api_key.trim().is_empty() {
        bail!("no API key configured for {}", opts.provider.label());
    }

    let ocr = match opts.provider {
        Provider::Mistral => mistral::ocr(&opts.api_key, pdf)?,
    };
    if ocr.markdown.trim().is_empty() {
        bail!("the OCR service returned no text for this PDF");
    }

    for (name, bytes) in &ocr.images {
        std::fs::write(workdir.join(name), bytes)
            .with_context(|| format!("failed to write OCR image {name}"))?;
    }
    let md_path = workdir.join("book.md");
    std::fs::write(&md_path, &ocr.markdown).context("failed to write OCR markdown")?;

    let md_opts = crate::markdown::ImportOptions {
        language: opts.language.clone(),
    };
    crate::markdown::import(&md_path, output, &md_opts)
}
