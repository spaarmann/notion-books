#![feature(let_else)]
#![feature(iterator_try_collect)]

mod gbooks;
mod notion;

use gbooks::GBook;
use miette::{Context, IntoDiagnostic, Result};
use std::io::Write;

use crate::{
    gbooks::GBooks,
    notion::{Notion, NotionBookEntry},
};

#[derive(knuffel::Decode)]
struct Config {
    #[knuffel(child, unwrap(argument))]
    google_books_api_key: String,
    #[knuffel(child, unwrap(argument))]
    notion_integration_token: String,
    #[knuffel(child, unwrap(argument))]
    notion_database_id: String,
}

fn read_stdin_line() -> Result<String> {
    std::io::stdout().flush().into_diagnostic()?;
    let mut buf = String::new();
    std::io::stdin().read_line(&mut buf).into_diagnostic()?;
    buf.truncate(buf.trim_end().len());
    Ok(buf)
}

fn read_config() -> Result<Config> {
    let path = "./config.kdl";
    let text = std::fs::read_to_string(path)
        .into_diagnostic()
        .wrap_err_with(|| format!("Failed to read file {}", path))?;
    let config = knuffel::parse::<Config>(path, &text)?;
    Ok(config)
}

#[tokio::main]
async fn main() -> Result<()> {
    let config = read_config().wrap_err("Failed to read configuration file")?;
    let gbooks = GBooks::new(config.google_books_api_key);

    let notion = Notion::new(config.notion_integration_token);
    let database = notion.database(config.notion_database_id).await?;

    print!("Enter query: ");
    let query = read_stdin_line()?;

    let search_results = gbooks
        .search(&query)
        .await
        .wrap_err("Failed to search on Google Books")?
        .collect::<Vec<_>>();

    let chosen_idx = if search_results.len() == 1 {
        0
    } else {
        println!("Choose book:");
        for (i, book) in search_results.iter().enumerate() {
            println!("{i}: {book}");
        }

        print!("> ");
        read_stdin_line()?
            .parse::<usize>()
            .into_diagnostic()
            .wrap_err("Invalid result index")?
    };

    let gbook = &search_results[chosen_idx];
    let query_results = database.search(&gbook.title).await?;

    enum Action {
        CreateNew,
        Update(usize),
    }

    let action = if query_results.len() > 0 {
        println!("Choose what you want to do:");
        println!("0: Create a new entry");
        for (i, entry) in query_results.iter().enumerate() {
            println!("{}: Update {entry}", i + 1);
        }
        let choice = read_stdin_line()?
            .parse::<usize>()
            .into_diagnostic()
            .wrap_err("Invalid choice")?;
        if choice == 0 {
            Action::CreateNew
        } else {
            Action::Update(choice - 1)
        }
    } else {
        println!("No matching entries found. Create new? (Y/N)");
        let choice = read_stdin_line()?;
        match choice.as_str() {
            "Y" | "y" | "Yes" | "yes" => Action::CreateNew,
            _ => return Ok(()),
        }
    };

    match action {
        Action::CreateNew => {
            let entry = create_notion_entry_from_gbook(gbook);
            database
                .add_entry(entry)
                .await
                .wrap_err("Failed to add new entry")?;
        }
        Action::Update(entry_idx) => {
            let mut entry_to_update = query_results[entry_idx].clone();
            update_notion_entry_from_gbook(&mut entry_to_update, gbook);
            database
                .update_entry(entry_to_update)
                .await
                .wrap_err("Failed to update entry")?;
        }
    }

    Ok(())
}

fn create_notion_entry_from_gbook(gbook: &GBook) -> NotionBookEntry {
    NotionBookEntry {
        id: None,
        title: gbook.title.clone(),
        authors: gbook.authors.clone(),
        author_ids: vec![None; gbook.authors.len()],
        publisher: gbook.publisher.clone(),
        publisher_id: None,
        published_date: gbook.published_date.clone(),
        isbn: gbook.isbn.clone(),
        description: gbook.description.clone(),
        had_original_description: false,
    }
}

fn update_notion_entry_from_gbook(entry_to_update: &mut NotionBookEntry, gbook: &GBook) {
    if entry_to_update.authors.is_empty() {
        entry_to_update.authors = gbook.authors.clone();
        entry_to_update.author_ids = vec![None; entry_to_update.authors.len()];
    }

    if entry_to_update.publisher.is_none() {
        entry_to_update.publisher = gbook.publisher.clone();
        entry_to_update.publisher_id = None;
    }

    if entry_to_update.published_date.is_none() {
        entry_to_update.published_date = gbook.published_date.clone();
    }

    if entry_to_update.isbn.is_none() {
        entry_to_update.isbn = gbook.isbn.clone();
    }

    if !entry_to_update.had_original_description {
        entry_to_update.description = gbook.description.clone();
    }
}
