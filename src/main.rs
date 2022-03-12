use reqwest::Response;
use serde_derive::Deserialize;
use std::{collections::HashMap, error::Error, future::Future, io::Write};
use url::Url;

const BOOKS_API_KEY: &'static str = include_str!("../books_api_key.txt");

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct GBookSearchResult {
    id: String,
    volume_info: GBookVolumeInfo,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct GBookVolumeInfo {
    authors: Option<Vec<String>>,
    image_links: Option<GBookImageLinks>,
    published_date: Option<String>,
    publisher: Option<String>,
    title: String,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct GBookImageLinks {
    thumbnail: String,
}

fn make_gbook_url(url: &str) -> Result<Url, url::ParseError> {
    Url::parse(&format!(
        "https://www.googleapis.com/books/v1/{}&key={}",
        url, BOOKS_API_KEY,
    ))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let client = reqwest::Client::new();

    print!("Enter query: ");
    std::io::stdout().flush()?;
    let mut buf = String::new();
    std::io::stdin().read_line(&mut buf)?;

    let resp = client
        .get(make_gbook_url(
            //"volumes?q=isbn:9780552147682&projection=lite",
            //"volumes?q=truth pratchett&projection=lite",
            &format!("volumes?q={}&projection=lite", buf),
        )?)
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;

    let search_results: Vec<GBookSearchResult> =
        serde_json::from_value(resp.get("items").unwrap().clone())?;

    let chosen_id = if search_results.len() == 1 {
        &search_results[0].id
    } else {
        println!("Choose book:");
        for (i, elem) in search_results.iter().enumerate() {
            println!(
                "{i}: {title} by {authors} ({publisher}, {published_date})",
                title = elem.volume_info.title,
                authors = elem
                    .volume_info
                    .authors
                    .as_ref()
                    .map(|authors| authors.join(", "))
                    .unwrap_or(String::new()),
                publisher = elem
                    .volume_info
                    .publisher
                    .as_ref()
                    .map(|s| s.as_str())
                    .unwrap_or(""),
                published_date = elem
                    .volume_info
                    .published_date
                    .as_ref()
                    .map(|s| s.as_str())
                    .unwrap_or("")
            );
        }

        print!("> ");
        std::io::stdout().flush()?;
        let mut buf = String::new();
        std::io::stdin().read_line(&mut buf)?;
        &search_results[buf.trim().parse::<usize>()?].id
    };

    println!("ID: {chosen_id}");

    //println!("{:#?}", search_results);

    let resp = client
        .get(make_gbook_url(&format!("volumes/{}?", chosen_id))?)
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;
    println!("{:#?}", resp);

    Ok(())
}
