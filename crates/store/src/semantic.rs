// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use rusqlite::{Connection, OptionalExtension, params};

use crate::StoreError;

const EMBEDDING_DIMENSIONS: usize = 256;
const CHUNK_SIZE: usize = 1200;
const CHUNK_OVERLAP: usize = 200;

#[derive(Debug, Clone)]
pub struct MemoryHit {
    pub source_kind: String,
    pub source_key: String,
    pub chunk_index: usize,
    pub content: String,
    pub score: f32,
}

pub fn ensure_schema(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS memory_documents (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            source_kind TEXT NOT NULL,
            source_key TEXT NOT NULL,
            content_hash TEXT NOT NULL,
            metadata_json TEXT NOT NULL,
            updated_at_ns INTEGER NOT NULL,
            UNIQUE(source_kind, source_key)
        );
        CREATE TABLE IF NOT EXISTS memory_chunks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            document_id INTEGER NOT NULL,
            chunk_index INTEGER NOT NULL,
            content TEXT NOT NULL,
            embedding_json TEXT NOT NULL,
            FOREIGN KEY(document_id) REFERENCES memory_documents(id) ON DELETE CASCADE,
            UNIQUE(document_id, chunk_index)
        );
        CREATE INDEX IF NOT EXISTS idx_memory_documents_source
            ON memory_documents(source_kind, source_key);
        CREATE INDEX IF NOT EXISTS idx_memory_chunks_document
            ON memory_chunks(document_id, chunk_index);",
    )?;
    Ok(())
}

pub fn upsert_document(
    conn: &Connection,
    source_kind: &str,
    source_key: &str,
    content: &str,
    metadata_json: &str,
    updated_at_ns: u64,
) -> Result<(), StoreError> {
    if content.trim().is_empty() {
        return Ok(());
    }

    let content_hash = stable_hash(content);
    let existing: Option<(i64, String)> = conn
        .query_row(
            "SELECT id, content_hash FROM memory_documents WHERE source_kind = ?1 AND source_key = ?2",
            params![source_kind, source_key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;

    let document_id = match existing {
        Some((id, existing_hash)) if existing_hash == content_hash => return Ok(()),
        Some((id, _)) => {
            conn.execute(
                "UPDATE memory_documents
                    SET content_hash = ?1, metadata_json = ?2, updated_at_ns = ?3
                  WHERE id = ?4",
                params![content_hash, metadata_json, updated_at_ns as i64, id],
            )?;
            conn.execute(
                "DELETE FROM memory_chunks WHERE document_id = ?1",
                params![id],
            )?;
            id
        }
        None => {
            conn.execute(
                "INSERT INTO memory_documents (source_kind, source_key, content_hash, metadata_json, updated_at_ns)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    source_kind,
                    source_key,
                    content_hash,
                    metadata_json,
                    updated_at_ns as i64
                ],
            )?;
            conn.last_insert_rowid()
        }
    };

    for (index, chunk) in chunk_text(content).into_iter().enumerate() {
        let embedding = embedding_for_text(&chunk);
        conn.execute(
            "INSERT INTO memory_chunks (document_id, chunk_index, content, embedding_json)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                document_id,
                index as i64,
                chunk,
                serde_json::to_string(&embedding)?
            ],
        )?;
    }

    Ok(())
}

pub fn search(conn: &Connection, query: &str, limit: usize) -> Result<Vec<MemoryHit>, StoreError> {
    let query_embedding = embedding_for_text(query);
    let mut stmt = conn.prepare(
        "SELECT d.source_kind, d.source_key, c.chunk_index, c.content, c.embedding_json
           FROM memory_chunks c
           JOIN memory_documents d ON d.id = c.document_id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;

    let mut hits = Vec::new();
    for row in rows {
        let (source_kind, source_key, chunk_index, content, embedding_json) = row?;
        let embedding: Vec<f32> = serde_json::from_str(&embedding_json)?;
        let score = cosine_similarity(&query_embedding, &embedding);
        hits.push(MemoryHit {
            source_kind,
            source_key,
            chunk_index: chunk_index as usize,
            content,
            score,
        });
    }

    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hits.truncate(limit);
    Ok(hits)
}

pub fn observation_text(
    origin_json: &str,
    kind_tag: &str,
    data_json: &str,
    source_file: Option<&str>,
) -> String {
    let mut text = format!("{origin_json}\n{kind_tag}\n{data_json}");
    if let Some(file) = source_file {
        text.push('\n');
        text.push_str(file);
    }
    text
}

fn chunk_text(content: &str) -> Vec<String> {
    let chars: Vec<char> = content.chars().collect();
    if chars.len() <= CHUNK_SIZE {
        return vec![content.to_string()];
    }

    let mut chunks = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let end = usize::min(start + CHUNK_SIZE, chars.len());
        let chunk: String = chars[start..end].iter().collect();
        chunks.push(chunk);
        if end == chars.len() {
            break;
        }
        start = end.saturating_sub(CHUNK_OVERLAP);
    }
    chunks
}

fn embedding_for_text(text: &str) -> Vec<f32> {
    let mut embedding = vec![0.0f32; EMBEDDING_DIMENSIONS];
    for token in text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
    {
        let mut hasher = DefaultHasher::new();
        token.to_ascii_lowercase().hash(&mut hasher);
        let hash = hasher.finish() as usize;
        let index = hash % EMBEDDING_DIMENSIONS;
        let sign = if (hash / EMBEDDING_DIMENSIONS).is_multiple_of(2) {
            1.0
        } else {
            -1.0
        };
        embedding[index] += sign;
    }

    let norm = embedding
        .iter()
        .map(|value| value * value)
        .sum::<f32>()
        .sqrt();
    if norm > 0.0 {
        for value in &mut embedding {
            *value /= norm;
        }
    }
    embedding
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> f32 {
    left.iter()
        .zip(right.iter())
        .map(|(a, b)| a * b)
        .sum::<f32>()
}

fn stable_hash(content: &str) -> String {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_prefers_related_chunks() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_schema(&conn).unwrap();

        upsert_document(
            &conn,
            "file",
            "src/main.rs",
            "tokio runtime spawn background agent and stream observations",
            "{}",
            1,
        )
        .unwrap();
        upsert_document(
            &conn,
            "file",
            "README.md",
            "marketing homepage with gradients and typography",
            "{}",
            1,
        )
        .unwrap();

        let hits = search(&conn, "background agent runtime", 2).unwrap();
        assert_eq!(hits[0].source_key, "src/main.rs");
    }
}
