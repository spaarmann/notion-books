mod gbooks;

use eyre::{Context, Result};
use std::io::Write;

use crate::gbooks::GBooks;

fn read_stdin_line() -> Result<String, std::io::Error> {
    std::io::stdout().flush()?;
    let mut buf = String::new();
    std::io::stdin().read_line(&mut buf)?;
    buf.truncate(buf.trim_end().len());
    Ok(buf)
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;

    let gbooks = GBooks::new(include_str!("../books_api_key.txt").to_string());

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
            .wrap_err("Invalid result index")?
    };

    println!("{:#?}", search_results[chosen_idx]);

    Ok(())
}
