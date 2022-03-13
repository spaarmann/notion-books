mod gbooks;

use miette::{Context, IntoDiagnostic, Result};
use std::io::Write;

use crate::gbooks::GBooks;

#[derive(knuffel::Decode)]
struct Config {
    #[knuffel(child, unwrap(argument))]
    google_books_api_key: String,
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
            println!("{i}: {}", book);
        }

        print!("> ");
        read_stdin_line()?
            .parse::<usize>()
            .into_diagnostic()
            .wrap_err("Invalid result index")?
    };

    println!("{:#?}", search_results[chosen_idx]);

    Ok(())
}
