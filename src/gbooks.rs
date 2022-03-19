use futures::future;
use miette::{miette, Context, IntoDiagnostic, Result};
use reqwest::{Client, Method, RequestBuilder};
use serde_derive::Deserialize;
use serde_json::Value;
use std::fmt::Display;
use url::{form_urlencoded::Serializer, Url, UrlQuery};

pub struct GBooks {
    api_key: String,
    client: Client,
}

#[derive(Debug, Clone)]
pub struct GBook {
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

    async fn request<U, R>(&self, method: Method, endpoint: &str, u: U, r: R) -> Result<Value>
    where
        U: FnOnce(&mut Serializer<'_, UrlQuery<'_>>),
        R: FnOnce(RequestBuilder) -> RequestBuilder,
    {
        let url = {
            let mut url =
                Url::parse(&format!("https://www.googleapis.com/books/v1{endpoint}")).unwrap();
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("key", &self.api_key);
            u(&mut pairs);
            drop(pairs);
            url
        };

        let default_request = self.client.request(method, url);
        let request = r(default_request);

        let response = request
            .send()
            .await
            .into_diagnostic()
            .wrap_err("Failed to send GBooks API request")?;

        let status = response.status();
        let response_body = response
            .json::<Value>()
            .await
            .into_diagnostic()
            .wrap_err("Failed to read GBooks API response")?;

        if !status.is_success() {
            return Err(miette!("Error {}:\n{:#?}", status, response_body));
        }

        Ok(response_body)
    }

    pub async fn search(&self, query: &str) -> Result<impl Iterator<Item = GBook>> {
        let response = self
            .request(
                Method::GET,
                "/volumes",
                |url| {
                    url.append_pair("projection", "lite")
                        .append_pair("q", query);
                },
                |req| req,
            )
            .await
            .wrap_err("Failed to search on Google Books")?;

        let volumes = future::try_join_all(
            response["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item["id"].as_str().unwrap().to_string())
                .map(|id| self.get(id)),
        )
        .await
        .wrap_err("Failed to retrieve detailed Google Books search result information")?;

        Ok(volumes.into_iter().map(|volume| {
            let isbn = volume.volume_info.get_isbn();
            GBook {
                title: volume.volume_info.title,
                authors: volume.volume_info.authors.unwrap_or_default(),
                publisher: volume.volume_info.publisher,
                published_date: volume.volume_info.published_date,
                description: volume.volume_info.description,
                isbn,
                image_link: volume
                    .volume_info
                    .image_links
                    .and_then(|links| links.into_largest_image()),
            }
        }))
    }

    async fn get(&self, id: String) -> Result<SearchResult> {
        let response = self
            .request(
                Method::GET,
                &format!("/volumes/{}", id),
                |_url| (),
                |req| req,
            )
            .await?;

        Ok(serde_json::from_value(response)
            .into_diagnostic()
            .wrap_err("Failed to deserialized GBooks API response")?)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchResult {
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
    industry_identifiers: Option<Vec<IndustryIdentifier>>,
    image_links: Option<ImageLinks>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct ImageLinks {
    small_thumbnail: Option<String>,
    thumbnail: Option<String>,
    small: Option<String>,
    medium: Option<String>,
    large: Option<String>,
    extra_large: Option<String>,
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
        if let Some(ids) = &self.industry_identifiers {
            for id in ids {
                if id.ty == "ISBN_13" {
                    return Some(id.identifier.clone());
                }
            }
        }
        None
    }
}

impl ImageLinks {
    fn into_largest_image(self) -> Option<String> {
        self.extra_large
            .or(self.large)
            .or(self.medium)
            .or(self.small)
            .or(self.thumbnail)
            .or(self.small_thumbnail)
    }
}
