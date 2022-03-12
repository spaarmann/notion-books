use eyre::{Context, Result};
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
            .wrap_err("Failed to send search request to Google Books")?
            .json::<serde_json::Value>()
            .await
            .wrap_err("Failed to read or parse Google Books response")?;

        let search_results: Vec<GBookSearchResult> =
            serde_json::from_value(response.get("items").unwrap().clone())
                .wrap_err("Failed to deserialize Google Books response")?;

        Ok(search_results.into_iter().map(|res| GBook {
            id: res.id,
            title: res.volume_info.title,
            authors: res.volume_info.authors.unwrap_or_default(),
            publisher: res.volume_info.publisher,
            published_date: res.volume_info.published_date,
            image_link: res.volume_info.image_links.map(|links| links.thumbnail),
        }))
    }
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct GBookSearchResult {
    id: String,
    volume_info: GBookVolumeInfo,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct GBookVolumeInfo {
    title: String,
    authors: Option<Vec<String>>,
    publisher: Option<String>,
    published_date: Option<String>,
    image_links: Option<GBookImageLinks>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct GBookImageLinks {
    thumbnail: String,
}
