use std::fmt::Display;

use futures::future;
use miette::{miette, IntoDiagnostic, Result, WrapErr};
use reqwest::{Client, Method, RequestBuilder};
use serde_json::{json, Map, Value};
use url::Url;

use crate::descriptions::{RichText, TextFragment};

#[derive(Debug)]
pub struct Notion {
    integration_token: String,
    client: Client,
}

#[derive(Debug)]
pub struct Database<'notion> {
    notion: &'notion Notion,
    database_id: String,
}

#[derive(Debug, Clone)]
pub struct NotionBookEntry {
    pub id: Option<String>,
    pub title: String,
    pub owned: bool,
    pub authors: Vec<String>,
    pub publisher: Option<String>,
    pub published_date: Option<String>,
    pub isbn: Option<String>,
    pub cover_url: Option<String>,
    pub author_ids: Vec<Option<String>>,
    pub publisher_id: Option<String>,

    // Description is special in that we do not have sufficient code to correctly read a whole
    // page body and set it again when editing an entry, since we only support setting a single
    // block with limited markup (and don't even pretend to support *getting* a description
    // properly).
    // To avoid deleting data, only ever *set* a description when editing an entry, if there was
    // no page body at all before.
    pub had_original_description: bool,
    pub description: Option<RichText>,
}

impl Notion {
    pub fn new(integration_token: String) -> Self {
        Self {
            integration_token,
            client: Client::new(),
        }
    }

    pub async fn database(&self, database_id: String) -> Result<Database<'_>> {
        Database::get(self, database_id).await
    }

    async fn request<F>(&self, method: Method, endpoint: &str, f: F) -> Result<Value>
    where
        F: FnOnce(RequestBuilder) -> RequestBuilder,
    {
        let url = Url::parse(&format!("https://api.notion.com/v1{endpoint}")).unwrap();

        let default_request = self
            .client
            .request(method, url)
            .header(
                "Authorization",
                format!("Bearer {}", self.integration_token),
            )
            .header("Content-Type", "application/json")
            .header("Notion-Version", "2022-02-22");
        let request = f(default_request);

        let response = request
            .send()
            .await
            .into_diagnostic()
            .wrap_err("Failed to send Notion API request")?;

        let status = response.status();
        let response_body = response
            .json::<Value>()
            .await
            .into_diagnostic()
            .wrap_err("Failed to read Notion API response")?;

        if !status.is_success() {
            return Err(miette!("Error {}:\n{:#?}", status, response_body));
        }

        Ok(response_body)
    }
}

impl<'notion> Database<'notion> {
    async fn get(notion: &'notion Notion, database_id: String) -> Result<Database<'notion>> {
        // This is async so that we could potentially grab some metadata about the database and its
        // schema, or just check if it exists, or similar.
        // We don't currently do any of those though.

        //let response = notion
        //    .request(Method::GET, &format!("/databases/{}", database_id), |req| {
        //        req
        //    })
        //    .await?;

        Ok(Self {
            notion,
            database_id,
        })
    }

    pub async fn search(&self, title: &str) -> Result<Vec<NotionBookEntry>> {
        let body = json!({
            "filter": {
                "and": [{
                    "property": "title",
                    "title": {
                        "contains": title
                    }
                }]
            }
        });

        let response = self
            .notion
            .request(
                Method::POST,
                &format!("/databases/{}/query", self.database_id),
                |req| req.json(&body),
            )
            .await?;

        let response = response["results"]
            .as_array()
            .ok_or_else(|| miette!("No results array in Notion API response!"))?;

        let results: Vec<NotionBookEntry> = response
            .into_iter()
            .map(|res| res.try_into())
            .try_collect()?;

        let results =
            future::try_join_all(results.into_iter().map(|entry| self.get_description(entry)))
                .await
                .wrap_err("Failed to get description for page!")?;

        Ok(results)
    }

    async fn get_description(&self, mut entry: NotionBookEntry) -> Result<NotionBookEntry> {
        let id = entry
            .id
            .as_ref()
            .ok_or_else(|| miette!("Tried to retrieve description for entry without ID!"))?;
        let response = self
            .notion
            .request(Method::GET, &format!("/blocks/{}/children", id), |req| req)
            .await?;

        let results = response["results"]
            .as_array()
            .ok_or_else(|| miette!("Get blocks API response has no results!"))?;

        if let Some(_) = results.first() {
            entry.had_original_description = true;
        }

        Ok(entry)
    }

    async fn set_description(&self, id: String, description: &RichText) -> Result<()> {
        let body = json!({ "children": [rich_text_to_block(description)] });

        self.notion
            .request(Method::PATCH, &format!("/blocks/{}/children", id), |req| {
                req.json(&body)
            })
            .await?;

        Ok(())
    }

    pub async fn add_entry(&self, book: NotionBookEntry) -> Result<()> {
        let description = book.description.clone();
        let cover_url = book.cover_url.clone();

        let mut body = json!({
            "parent": {
                "database_id": self.database_id
            },
            "properties": properties_from_entry(book)
        });

        if let Some(url) = cover_url {
            body.as_object_mut()
                .unwrap()
                .insert("cover".to_string(), json!({ "external": { "url": url } }));
        }

        let response = self
            .notion
            .request(Method::POST, "/pages/", |req| req.json(&body))
            .await?;

        if let Some(description) = description {
            let added_entry =
                NotionBookEntry::try_from(&response).wrap_err("Failed to parse added page")?;
            self.set_description(added_entry.id.unwrap(), &description)
                .await
                .wrap_err("Failed to set description for new entry!")?;
        }

        Ok(())
    }

    pub async fn update_entry(&self, book: NotionBookEntry) -> Result<()> {
        let id = book
            .id
            .clone()
            .ok_or_else(|| miette!("Tried to update entry but don't know ID"))?;

        let description_to_set = if book.had_original_description {
            None
        } else {
            book.description.clone()
        };

        let cover_url = book.cover_url.clone();

        let mut body = json!({ "properties": properties_from_entry(book) });

        if let Some(url) = cover_url {
            body.as_object_mut()
                .unwrap()
                .insert("cover".to_string(), json!({ "external": { "url": url } }));
        }

        self.notion
            .request(Method::PATCH, &format!("/pages/{}", id), |req| {
                req.json(&body)
            })
            .await?;

        if let Some(description) = description_to_set {
            self.set_description(id, &description)
                .await
                .wrap_err("Failed to set description!")?;
        }

        Ok(())
    }
}

impl TryFrom<&Value> for NotionBookEntry {
    type Error = miette::Error;

    fn try_from(value: &Value) -> Result<Self> {
        (|| -> Option<Self> {
            let props = &value["properties"];

            let authors = props["Authors"]["multi_select"]
                .as_array()?
                .iter()
                .map(|author| Some(author["name"].as_str()?.to_string()))
                .try_collect()?;
            let author_ids = props["Authors"]["multi_select"]
                .as_array()?
                .iter()
                .map(|author| Some(Some(author["id"].as_str()?.to_string())))
                .try_collect()?;

            let owned = props["Ownership"]["select"]
                .as_object()
                .map(|s| s["name"].as_str().unwrap() == "Own")
                .unwrap_or(false);

            Some(Self {
                id: Some(value["id"].as_str()?.to_string()),
                cover_url: value
                    .get("cover")
                    .filter(|c| c.is_object())
                    .map(|c| c["external"]["url"].as_str().unwrap().to_string()),
                title: props["Name"]["title"].as_array()?[0]["plain_text"]
                    .as_str()?
                    .to_string(),
                owned,
                authors,
                publisher: props["Publisher"]["select"]
                    .as_object()
                    .map(|obj| obj["name"].as_str().unwrap().to_string()),
                published_date: props["Publish Date"]["rich_text"]
                    .as_array()?
                    .get(0)
                    .map(|date| date["plain_text"].as_str().unwrap().to_string()),
                isbn: props["ISBN"]["rich_text"]
                    .as_array()?
                    .get(0)
                    .map(|isbn| isbn["plain_text"].as_str().unwrap().to_string()),
                //cover_url: None,
                author_ids,
                publisher_id: props["Publisher"]["select"]
                    .as_object()
                    .map(|obj| obj["id"].as_str().unwrap().to_string()),
                description: None,
                had_original_description: false,
            })
        })()
        .ok_or_else(|| miette!("Failed to parse database entry!"))
    }
}

fn properties_from_entry(entry: NotionBookEntry) -> Value {
    let mut properties = Map::<String, Value>::new();

    properties.insert("Type".to_string(), json!({ "select": { "name": "Book" } }));

    properties.insert(
        "Name".to_string(),
        json!({
            "title": [{
                "text": { "content": entry.title }
            }]
        }),
    );

    if entry.owned {
        properties.insert(
            "Ownership".to_string(),
            json!({
                "select": { "name": "Own" }
            }),
        );
    }

    let authors = entry
        .authors
        .into_iter()
        .zip(entry.author_ids.into_iter())
        .map(|(name, id)| match id {
            Some(id) => json!({ "id": id, "name": name }),
            None => json!({ "name": name }),
        })
        .collect::<Vec<_>>();

    if authors.len() > 0 {
        properties.insert("Authors".to_string(), json!({ "multi_select": authors }));
    }

    if let Some(publisher) = entry.publisher {
        let sanitized_name = publisher.replace(",", "");
        let value = match entry.publisher_id {
            Some(id) => json!({ "id": id, "name": sanitized_name }),
            None => json!({ "name": sanitized_name }),
        };
        properties.insert(
            "Publisher".to_string(),
            json!({
                "select": value,
            }),
        );
    }

    if let Some(date) = entry.published_date {
        properties.insert(
            "Publish Date".to_string(),
            json!({
                "rich_text": [{
                    "text": { "content": date }
                }]
            }),
        );
    }

    if let Some(isbn) = entry.isbn {
        properties.insert(
            "ISBN".to_string(),
            json!({
                "rich_text": [{
                    "text": { "content": isbn }
                }]
            }),
        );
    }

    Value::Object(properties)
}

fn rich_text_to_block(text: &RichText) -> Value {
    let mut val = Map::<String, Value>::new();

    val.insert("object".to_string(), Value::String("block".to_string()));
    val.insert("type".to_string(), Value::String("paragraph".to_string()));

    let make_rich_text = |frag: &TextFragment| {
        json!({
            "type": "text",
            "text": { "content": frag.text },
            "annotations": {
                "bold": frag.style.bold,
                "italic": frag.style.italic,
            },
        })
    };

    let paragraph = {
        let mut par = Map::<String, Value>::new();

        par.insert(
            "rich_text".to_string(),
            Value::Array(text.fragments.iter().map(make_rich_text).collect()),
        );

        Value::Object(par)
    };
    val.insert("paragraph".to_string(), paragraph);

    Value::Object(val)
}

impl Display for NotionBookEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{title} by {authors}",
            title = self.title,
            authors = self.authors.join(", "),
        )?;
        match (&self.publisher, &self.published_date) {
            (Some(publisher), Some(date)) => write!(f, " ({}, {})", publisher, date)?,
            (Some(publisher), None) => write!(f, " ({})", publisher)?,
            (None, Some(date)) => write!(f, " ({})", date)?,
            (None, None) => {}
        }
        if let Some(isbn) = &self.isbn {
            write!(f, " ({})", isbn)?;
        }
        Ok(())
    }
}
