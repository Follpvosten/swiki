use chrono::{DateTime, Utc};
use pulldown_cmark::{html, BrokenLink, CowStr, Event, Options, Parser, Tag};
use tantivy::{
    collector::TopDocs,
    doc,
    query::QueryParser,
    schema::{Field, Schema, STORED, TEXT},
    IndexReader, SnippetGenerator, Term, UserOperation,
};

use crate::{database::articles::Revision, Db, Result};

pub struct ArticleIndex {
    name_field: Field,
    content_field: Field,
    date_field: Field,
    inner: tantivy::Index,
    reader: IndexReader,
}

#[derive(serde::Serialize)]
pub struct SearchResult {
    pub title: String,
    pub snippet: String,
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
    output.shrink_to_fit();
    output
}

impl ArticleIndex {
    pub fn new(db: &Db) -> Result<ArticleIndex> {
        let mut schema_builder = Schema::builder();
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
            let mut article = SearchResult {
                title: String::default(),
                snippet: snippet_generator.snippet_from_doc(&doc).to_html(),
                last_edited: chrono::MIN_DATETIME,
            };
            for field in doc.field_values() {
                if field.field() == self.name_field {
                    if let Some(value) = field.value().text() {
                        article.title = value.to_string();
                    }
                } else if field.field() == self.date_field {
                    article.last_edited = *field.value().date_value();
                }
            }
            result.push(article);
        }
        Ok(result)
    }

    pub fn update_article(
        &self,
        article_name: &str,
        new_content: &str,
        new_date: DateTime<Utc>,
    ) -> Result<()> {
        let mut writer = self.inner.writer_with_num_threads(1, 3_000_000)?;
        writer.run(vec![
            UserOperation::Delete(Term::from_field_text(self.name_field, article_name)),
            UserOperation::Add(doc! {
                self.name_field => article_name,
                self.content_field => markdown_to_text(new_content),
                self.date_field => new_date
            }),
        ]);
        writer.commit()?;
        Ok(())
    }

    pub fn rename_article(
        &self,
        old_name: &str,
        new_name: &str,
        content: &str,
        last_edited: DateTime<Utc>,
    ) -> Result<()> {
        let mut writer = self.inner.writer_with_num_threads(1, 3_000_000)?;
        writer.run(vec![
            UserOperation::Delete(Term::from_field_text(self.name_field, old_name)),
            UserOperation::Add(doc! {
                self.name_field => new_name,
                self.content_field => markdown_to_text(content),
                self.date_field => last_edited
            }),
        ]);
        writer.commit()?;
        Ok(())
    }
}
