use chrono::{DateTime, Utc};
use pulldown_cmark::{html, BrokenLink, CowStr, Event, Options, Parser, Tag};
use tantivy::{
    collector::TopDocs,
    doc,
    query::QueryParser,
    schema::{Field, Schema, INDEXED, STORED, TEXT},
    IndexReader, Snippet, SnippetGenerator, Term,
};

use crate::{database::articles::ArticleId, Result};

pub struct ArticleIndex {
    id_field: Field,
    name_field: Field,
    content_field: Field,
    date_field: Field,
    inner: tantivy::Index,
    reader: IndexReader,
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
impl SnippetOrFirstSentence {
    #[cfg(test)]
    fn inner_str(&self) -> &str {
        match self {
            Self::Snippet(s) => s.fragments(),
            Self::FirstSentence(s) => s.as_str(),
        }
    }
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
    pub fn new(db: &crate::Db) -> Result<ArticleIndex> {
        use crate::database::articles::Revision;

        let mut schema_builder = Schema::builder();
        let id_field = schema_builder.add_u64_field("id", INDEXED);
        let name_field = schema_builder.add_text_field("name", TEXT | STORED);
        let content_field = schema_builder.add_text_field("content", TEXT | STORED);
        let date_field = schema_builder.add_date_field("last_edited", STORED);
        let schema = schema_builder.build();
        let inner = tantivy::Index::create_in_ram(schema);

        let mut writer = inner.writer(50_000_000)?;
        for article_id in db.articles.list_articles()? {
            let article_name = db
                .articles
                .name_by_id(article_id)?
                .expect("Inconsistent data: name for article_id not found");
            let (
                _,
                Revision {
                    author_id: _,
                    content: article_content,
                    date,
                },
            ) = db
                .articles
                .get_current_revision(article_id)?
                .expect("Inconsistent data: article_id not found");
            writer.add_document(doc! {
                id_field => article_id.0 as u64,
                name_field => article_name,
                content_field => markdown_to_text(&article_content),
                date_field => date
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
        id: ArticleId,
        article_name: &str,
        content: &str,
        date: DateTime<Utc>,
    ) -> Result<()> {
        let mut writer = self.inner.writer_with_num_threads(1, 3_000_000)?;
        writer.delete_term(Term::from_field_u64(self.id_field, id.0 as _));
        writer.add_document(doc! {
            self.id_field => id.0 as u64,
            self.name_field => article_name,
            self.content_field => markdown_to_text(content),
            self.date_field => date
        });
        writer.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::Utc;

    use super::{markdown_to_text, ArticleIndex};
    use crate::{Db, Result};

    #[test]
    fn index_from_db() -> Result<()> {
        // Generate a mocked database with some articles.
        let db = Db::load_or_create(sled::Config::default().temporary(true).open()?)?;
        let author_id = db.users.register("User1", "12345")?;
        // The articles are randomly generated and stored.
        let mut names_to_contents: HashMap<String, String> = HashMap::with_capacity(128);
        for _ in 0..128 {
            // We want names to be unique
            let name = loop {
                let name = lipsum::lipsum_title();
                if !names_to_contents.contains_key(&name) {
                    break name;
                }
            };
            let article_id = db.articles.create(&name)?;
            let content = lipsum::lipsum_words(100);
            db.articles.add_revision(article_id, author_id, &content)?;
            names_to_contents.insert(name, markdown_to_text(&content));
        }
        let index = ArticleIndex::new(&db)?;
        for (name, content) in names_to_contents {
            let results = index.search_by_text(&name)?;
            let specific_result = results
                .into_iter()
                .find(|res| res.title == name)
                .expect("article not found with exact name");
            assert!(
                dbg!(content).contains(dbg!(specific_result.snippet.inner_str())),
                "article content not right"
            )
        }
        Ok(())
    }

    #[test]
    fn add_update_and_rename_works() -> Result<()> {
        let empty_db = Db::load_or_create(sled::Config::default().temporary(true).open()?)?;
        let index = ArticleIndex::new(&empty_db)?;
        let article_id = empty_db.articles.create("blah blah")?;
        let name = "Lorem Ipsum";
        let text = "This is a fun short text that should be very texty.";
        // Check if an empty db works at all
        assert_eq!(index.search_by_text(name)?.len(), 0);
        // Add an article
        index.add_or_update_article(article_id, name, text, Utc::now())?;
        // Force the indexreader to reload.
        index.reader.reload()?;
        // Verify we can find it
        assert_eq!(dbg!(index.search_by_text(name)?).len(), 1);
        // Rename the article
        let new_name = "Baumhardt 123";
        index.add_or_update_article(article_id, new_name, text, Utc::now())?;
        index.reader.reload()?;
        // Check if the old name yields no results, but the new one does
        assert_eq!(dbg!(index.search_by_text(new_name)?).len(), 1);
        assert_eq!(dbg!(index.search_by_text(name)?).len(), 0);
        Ok(())
    }
}
