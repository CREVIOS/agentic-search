//! Optional BM25 (tantivy) index for unstructured corpora.
//!
//! This crate is NOT on the default agentic hot path. The 2026 consensus
//! is that grep + AST spans dominates retrieval for code-shaped agent
//! workloads; tantivy is here for the non-code cases (PDF prose, support
//! tickets, scraped HTML, …) where lexical recall genuinely helps.

use as_core::{Doc, Error, Hit, Result};
use std::path::Path;
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{Field, Schema, Value, STORED, TEXT};
use tantivy::{doc, Index, IndexWriter, TantivyDocument};

pub struct TantivyIndex {
    index: Index,
    id_field: Field,
    uri_field: Field,
    text_field: Field,
}

impl TantivyIndex {
    pub fn open_or_create(path: &Path) -> Result<Self> {
        let mut schema = Schema::builder();
        let id_field = schema.add_text_field("id", STORED | TEXT);
        let uri_field = schema.add_text_field("uri", STORED | TEXT);
        let text_field = schema.add_text_field("text", TEXT | STORED);
        let schema = schema.build();

        std::fs::create_dir_all(path).map_err(Error::Io)?;
        let index = Index::open_or_create(
            tantivy::directory::MmapDirectory::open(path)
                .map_err(|e| Error::Index(e.to_string()))?,
            schema,
        )
        .map_err(|e| Error::Index(e.to_string()))?;

        Ok(Self {
            index,
            id_field,
            uri_field,
            text_field,
        })
    }

    pub fn writer(&self, mem_mb: usize) -> Result<IndexWriter> {
        self.index
            .writer(mem_mb * 1024 * 1024)
            .map_err(|e| Error::Index(e.to_string()))
    }

    pub fn add(&self, writer: &mut IndexWriter, d: &Doc) -> Result<()> {
        writer
            .add_document(doc!(
                self.id_field => d.id.clone(),
                self.uri_field => d.uri.clone(),
                self.text_field => d.text.clone(),
            ))
            .map_err(|e| Error::Index(e.to_string()))?;
        Ok(())
    }

    pub fn search(&self, query: &str, k: usize) -> Result<Vec<Hit>> {
        let reader = self
            .index
            .reader()
            .map_err(|e| Error::Index(e.to_string()))?;
        let searcher = reader.searcher();
        let parser = QueryParser::for_index(&self.index, vec![self.text_field]);
        let q = parser
            .parse_query(query)
            .map_err(|e| Error::Index(e.to_string()))?;
        let top = searcher
            .search(&q, &TopDocs::with_limit(k))
            .map_err(|e| Error::Index(e.to_string()))?;

        let mut hits = Vec::with_capacity(top.len());
        for (score, addr) in top {
            let retrieved: TantivyDocument = searcher
                .doc(addr)
                .map_err(|e| Error::Index(e.to_string()))?;
            let id = retrieved
                .get_first(self.id_field)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let uri = retrieved
                .get_first(self.uri_field)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let snippet = retrieved
                .get_first(self.text_field)
                .and_then(|v| v.as_str())
                .map(|s| s.chars().take(240).collect::<String>());
            hits.push(Hit {
                id,
                uri,
                score,
                snippet,
                metadata: serde_json::Value::Null,
            });
        }
        Ok(hits)
    }
}
