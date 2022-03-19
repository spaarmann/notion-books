use std::fmt::Display;

use miette::{miette, IntoDiagnostic, Result, WrapErr};
use reqwest::{Client, Method, RequestBuilder};
use serde_json::{json, Map, Value};
use url::Url;

#[derive(Debug)]
pub struct Notion {
    integration_token: String,
    client: Client,
}

#[derive(Clone, Debug)]
pub struct StringOption {
    id: String,
    name: String,
}

#[derive(Debug)]
pub struct Database<'notion> {
    notion: &'notion Notion,
    database_id: String,
    publishers: Vec<StringOption>,
    authors: Vec<StringOption>,
}

#[derive(Debug, Clone)]
pub struct NotionBookEntry {
    pub id: Option<String>,
    pub title: String,
    pub authors: Vec<String>,
    pub publisher: Option<String>,
    pub published_date: Option<String>,
    pub isbn: Option<String>,
    pub cover_url: Option<String>,
    pub author_ids: Vec<Option<String>>,
    pub publisher_id: Option<String>,

    // Description is special in that we do not have sufficient code to correctly read a whole
    // page body and set it again when editing an entry, since we pretend it's just a simple
    // string (rather than a list of blocks).
    // To avoid deleting data, only ever *set* a description when editing an entry, if there was
    // no page body at all before.
    pub had_original_description: bool,
    pub description: Option<String>,
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
        let response = notion
            .request(Method::GET, &format!("/databases/{}", database_id), |req| {
                req
            })
            .await?;

        let props = &response["properties"];

        let authors = &props["Authors"]["multi_select"]["options"];
        let Value::Array(authors) = authors else {
            return Err(miette!("Unexpected database schema!"));
        };
        let authors = authors
            .into_iter()
            .map(|author| -> Result<StringOption> {
                Ok(StringOption {
                    id: author["id"]
                        .as_str()
                        .ok_or_else(|| miette!("Unexpected database schema!"))?
                        .to_string(),
                    name: author["name"]
                        .as_str()
                        .ok_or_else(|| miette!("Unexpected database schema!"))?
                        .to_string(),
                })
            })
            .try_collect::<Vec<_>>()?;

        let publishers = &props["Publisher"]["select"]["options"];
        let Value::Array(publishers) = publishers else {
            return Err(miette!("Unexpected database schema!"));
        };
        let publishers = publishers
            .into_iter()
            .map(|publisher| -> Result<StringOption> {
                Ok(StringOption {
                    id: publisher["id"]
                        .as_str()
                        .ok_or_else(|| miette!("Unexpected database schema!"))?
                        .to_string(),
                    name: publisher["name"]
                        .as_str()
                        .ok_or_else(|| miette!("Unexpected database schema!"))?
                        .to_string(),
                })
            })
            .try_collect::<Vec<_>>()?;

        Ok(Self {
            notion,
            database_id,
            authors,
            publishers,
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

        let mut results = Vec::with_capacity(response.len());
        for res in response {
            let mut entry: NotionBookEntry = res.try_into()?;

            self.get_description(&mut entry)
                .await
                .wrap_err("Failed to get description for page!")?;

            results.push(entry);
        }

        Ok(results)
    }

    async fn get_description(&self, entry: &mut NotionBookEntry) -> Result<()> {
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

        if let Some(block) = results.first() {
            entry.had_original_description = true;

            if let Some(paragraph) = block.get("paragraph") {
                entry.description = Some(
                    paragraph["rich_text"][0]["plain_text"]
                        .as_str()
                        .unwrap()
                        .to_string(),
                );
            }
        }

        Ok(())
    }

    async fn set_description(&self, id: String, description: String) -> Result<()> {
        let body = json!({
            "children": [{
                "object": "block",
                "type": "paragraph",
                "paragraph": {
                    "rich_text": [{
                        "type": "text",
                        "text": { "content": description }
                    }]
                }
            }]
        });

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
            self.set_description(added_entry.id.unwrap(), description)
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
            self.set_description(id, description)
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

            Some(Self {
                id: Some(value["id"].as_str()?.to_string()),
                cover_url: value
                    .get("cover")
                    .filter(|c| c.is_object())
                    .map(|c| c["external"]["url"].as_str().unwrap().to_string()),
                title: props["Name"]["title"].as_array()?[0]["plain_text"]
                    .as_str()?
                    .to_string(),
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
        let value = match entry.publisher_id {
            Some(id) => json!({ "id": id, "name": publisher }),
            None => json!({ "name": publisher }),
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
