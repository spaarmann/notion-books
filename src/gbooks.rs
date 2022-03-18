use miette::{Context, IntoDiagnostic, Result};
use reqwest::Client;
use serde_derive::Deserialize;
use std::fmt::Display;
use url::Url;

pub struct GBooks {
    api_key: String,
    client: Client,
}

#[derive(Debug, Clone)]
pub struct GBook {
    id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub publisher: Option<String>,
    pub published_date: Option<String>,
    pub isbn: Option<String>,
    pub description: Option<String>,
    pub image_link: Option<String>,
}

impl Display for GBook {
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

impl GBooks {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            client: Client::new(),
        }
    }

    pub async fn search(&self, query: &str) -> Result<impl Iterator<Item = GBook>> {
        let url = {
            let mut url = Url::parse("https://www.googleapis.com/books/v1/volumes").unwrap();
            url.query_pairs_mut()
                .append_pair("key", &self.api_key)
                .append_pair("projection", "full")
                .append_pair("q", query);
            url
        };

        let response = self
            .client
            .get(url)
            .send()
            .await
            .into_diagnostic()
            .wrap_err("Failed to send search request to Google Books")?
            .error_for_status()
            .into_diagnostic()
            .wrap_err("Failed to search on Google Books")?
            .json::<serde_json::Value>()
            .await
            .into_diagnostic()
            .wrap_err("Failed to read or parse Google Books response")?;

        let search_results: Vec<SearchResult> =
            serde_json::from_value(response.get("items").unwrap().clone())
                .into_diagnostic()
                .wrap_err("Failed to deserialize Google Books response")?;

        Ok(search_results.into_iter().map(|res| {
            let isbn = res.volume_info.get_isbn();
            GBook {
                id: res.id,
                title: res.volume_info.title,
                authors: res.volume_info.authors.unwrap_or_default(),
                publisher: res.volume_info.publisher,
                published_date: res.volume_info.published_date,
                description: res.volume_info.description,
                isbn,
                image_link: res.volume_info.image_links.map(|links| links.thumbnail),
            }
        }))
    }
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct SearchResult {
    id: String,
    volume_info: VolumeInfo,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct VolumeInfo {
    title: String,
    authors: Option<Vec<String>>,
    publisher: Option<String>,
    published_date: Option<String>,
    description: Option<String>,
    industry_identifiers: Vec<IndustryIdentifier>,
    image_links: Option<ImageLinks>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct ImageLinks {
    thumbnail: String,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct IndustryIdentifier {
    #[serde(rename = "type")]
    ty: String,
    identifier: String,
}

impl VolumeInfo {
    fn get_isbn(&self) -> Option<String> {
        for id in &self.industry_identifiers {
            if id.ty == "ISBN_13" {
                return Some(id.identifier.clone());
            }
        }
        None
    }
}
