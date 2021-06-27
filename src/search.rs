use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use pulldown_cmark::{html, BrokenLink, CowStr, Event, Options, Parser, Tag};
use tantivy::{
    collector::TopDocs,
    doc,
    query::QueryParser,
    schema::{Field, Schema, STORED, STRING, TEXT},
    IndexReader, IndexWriter, Snippet, SnippetGenerator, Term,
};
use uuid::Uuid;

use crate::{db::articles::ArticleWithRevision, Result};

pub struct ArticleIndex {
    id_field: Field,
    name_field: Field,
    content_field: Field,
    date_field: Field,
    inner: tantivy::Index,
    pub(crate) reader: IndexReader,
    writer: Mutex<IndexWriter>,
}

fn serialize_snippet<S: serde::Serializer>(
    snippet: &SnippetOrFirstSentence,
    s: S,
) -> std::result::Result<S::Ok, S::Error> {
    match snippet {
        SnippetOrFirstSentence::Snippet(snippet) => s.serialize_str(&snippet.to_html()),
        SnippetOrFirstSentence::FirstSentence(string) => s.serialize_str(string),
    }
}

#[derive(Debug)]
pub enum SnippetOrFirstSentence {
    Snippet(Snippet),
    FirstSentence(String),
}

#[derive(Debug, serde::Serialize)]
pub struct SearchResult {
    pub title: String,
    #[serde(serialize_with = "serialize_snippet")]
    pub snippet: SnippetOrFirstSentence,
    pub last_edited: DateTime<Utc>,
}

fn markdown_to_text(input: &str) -> String {
    // TODO: This is pretty unnecessary since I actually just want to strip
    // the square brackets from broken links. Hm.
    let callback = &mut |broken_link: BrokenLink| {
        Some((
            ("/".to_string() + broken_link.reference).into(),
            broken_link.reference.to_owned().into(),
        ))
    };
    let parser = Parser::new_with_broken_link_callback(input, Options::all(), Some(callback))
        .filter_map(|event| match event {
            Event::Text(_) => Some(event),
            Event::Start(Tag::Link(_, _, _)) | Event::End(Tag::Link(_, _, _)) => None,
            _ => Some(Event::Text(CowStr::Borrowed(" "))),
        });
    // The output will very likely be shorter than the input, but never longer
    let mut output = String::with_capacity(input.len());
    html::push_html(&mut output, parser);
    output.trim().into()
}

impl ArticleIndex {
    pub async fn new(db: &crate::Db) -> Result<ArticleIndex> {
        let mut schema_builder = Schema::builder();
        let id_field = schema_builder.add_text_field("id", STRING);
        let name_field = schema_builder.add_text_field("name", TEXT | STORED);
        let content_field = schema_builder.add_text_field("content", TEXT | STORED);
        let date_field = schema_builder.add_date_field("last_edited", STORED);
        let schema = schema_builder.build();
        let inner = tantivy::Index::create_in_ram(schema);

        let mut writer = inner.writer(50_000_000)?;
        for article in db.list_articles().await? {
            let ArticleWithRevision {
                id,
                name,
                content,
                rev_created,
            } = article;
            let date = DateTime::from_utc(rev_created, Utc);
            writer.add_document(doc! {
                id_field => id.to_string(),
                name_field => name,
                content_field => markdown_to_text(&content),
                date_field => date,
            });
        }
        writer.commit()?;

        let reader = inner
            .reader_builder()
            .reload_policy(tantivy::ReloadPolicy::OnCommit)
            .try_into()?;

        Ok(ArticleIndex {
            id_field,
            name_field,
            content_field,
            date_field,
            inner,
            reader,
            writer: Mutex::new(writer),
        })
    }

    pub fn search_by_text(&self, text: &str) -> Result<Vec<SearchResult>> {
        let searcher = self.reader.searcher();
        let query_parser =
            QueryParser::for_index(&self.inner, vec![self.name_field, self.content_field]);
        let query = query_parser.parse_query(text)?;
        let snippet_generator = SnippetGenerator::create(&searcher, &*query, self.content_field)?;
        let top_docs = searcher.search(&query, &TopDocs::with_limit(10))?;

        let mut result = Vec::with_capacity(top_docs.len());
        for (_, doc_address) in top_docs {
            let doc = searcher.doc(doc_address)?;
            let snippet = snippet_generator.snippet_from_doc(&doc);
            let snippet = if snippet.fragments().is_empty() {
                doc.field_values()
                    .iter()
                    .find(|field| field.field() == self.content_field)
                    .and_then(|field| field.value().text())
                    .map(|content| {
                        content
                            .find(|c: char| c.is_ascii_punctuation() && c != ',')
                            .map(|index| usize::min(index + 1, content.len()))
                            .map(|index| &content[..index])
                            .unwrap_or(content)
                            .to_string()
                    })
                    .map(SnippetOrFirstSentence::FirstSentence)
                    .unwrap()
            } else {
                SnippetOrFirstSentence::Snippet(snippet)
            };
            let mut article = SearchResult {
                title: String::default(),
                snippet,
                last_edited: chrono::MIN_DATETIME,
            };
            for field in doc.field_values() {
                if field.field() == self.name_field {
                    if let Some(value) = field.value().text() {
                        article.title = value.to_string();
                    }
                } else if field.field() == self.date_field {
                    if let Some(value) = field.value().date_value() {
                        article.last_edited = *value;
                    }
                }
            }
            result.push(article);
        }
        Ok(result)
    }

    /// Unconditionally tries to remove the article with the given id and
    /// recreates it with the given parameters.
    ///
    /// Passing in a different name than the article had before will also
    /// rename it, making the old rename_article method redundant.
    pub fn add_or_update_article(
        &self,
        id: Uuid,
        article_name: &str,
        content: &str,
        date: DateTime<Utc>,
    ) -> Result<()> {
        let id = id.to_string();
        let mut writer = self.writer.lock();
        writer.delete_term(Term::from_field_text(self.id_field, &id));
        writer.add_document(doc! {
            self.id_field => id,
            self.name_field => article_name,
            self.content_field => markdown_to_text(content),
            self.date_field => date,
        });
        writer.commit()?;
        Ok(())
    }
}
