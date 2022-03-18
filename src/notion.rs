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
    //pub cover_url: Option<String>,
    //pub description: Option<String>,
    pub author_ids: Vec<Option<String>>,
    pub publisher_id: Option<String>,
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

        let results = response["results"]
            .as_array()
            .ok_or_else(|| miette!("No results array in Notion API response!"))?
            .into_iter()
            .map(|res| res.try_into())
            .try_collect()?;

        Ok(results)
    }

    pub async fn add_entry(&self, book: NotionBookEntry) -> Result<()> {
        let body = json!({
            "parent": {
                "database_id": self.database_id
            },
            "properties": properties_from_entry(book)
        });

        let response = self
            .notion
            .request(Method::POST, "/pages/", |req| req.json(&body))
            .await?;

        println!("Response: {}", response);
        Ok(())
    }

    pub async fn update_entry(&self, book: NotionBookEntry) -> Result<()> {
        let id = book
            .id
            .clone()
            .ok_or_else(|| miette!("Tried to update entry but don't know ID"))?;
        let body = json!({ "properties": properties_from_entry(book) });

        let response = self
            .notion
            .request(Method::PATCH, &format!("/pages/{}", id), |req| {
                req.json(&body)
            })
            .await?;

        println!("Response: {}", response);
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
                //description: None,
                author_ids,
                publisher_id: props["Publisher"]["select"]
                    .as_object()
                    .map(|obj| obj["id"].as_str().unwrap().to_string()),
            })
        })()
        .ok_or_else(|| miette!("Failed to parse database entry!"))
    }
}

fn properties_from_entry(entry: NotionBookEntry) -> Value {
    let mut properties = Map::<String, Value>::new();

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
