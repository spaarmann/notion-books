mod gbooks;

use std::{error::Error, io::Write};

use crate::gbooks::GBooks;

fn read_stdin_line() -> Result<String, std::io::Error> {
    std::io::stdout().flush()?;
    let mut buf = String::new();
    std::io::stdin().read_line(&mut buf)?;
    buf.truncate(buf.trim_end().len());
    Ok(buf)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let gbooks = GBooks::new(include_str!("../books_api_key.txt").to_string());

    print!("Enter query: ");
    let query = read_stdin_line()?;

    let search_results = gbooks.search(&query).await?.collect::<Vec<_>>();

    let chosen_idx = if search_results.len() == 1 {
        0
    } else {
        println!("Choose book:");
        for (i, book) in search_results.iter().enumerate() {
            println!("{i}: {}", book);
        }

        print!("> ");
        read_stdin_line()?.parse::<usize>()?
    };

    println!("{:#?}", search_results[chosen_idx]);

    Ok(())
}
