use std::{env::args, error::Error, fs::File};

use chrono;
use epub_builder::{EpubBuilder, EpubContent, ReferenceType, ZipLibrary};
use futures::{stream, StreamExt};
use hex;
use regex::Regex;
use reqwest;
use scraper::{Html, Selector};
use tokio;

struct Chapter {
    name: String,
    link: String,
    content: String,
}

impl Chapter {
    fn new(name: String, link: String) -> Self {
        Chapter {
            name,
            link: if !link.starts_with("http") { format!("https://{}", link) } else { link },
            content: String::new(),
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let num_threads: usize = args().nth(2).unwrap_or("8".to_string()).parse()?;
    let mut link = args().nth(1).expect("No link provided");
    if link.ends_with("/") {
        link.pop();
    }
    println!("Creating an ebook for: {}", &link);

    print!("Getting table of contents...");
    let chapters = get_chapter_list(&link).await?;
    println!("done!");

    println!("Downloading chapters... this might take a while.");
    let mut handles = Vec::with_capacity(chapters.len());
    for chapter in chapters {
        handles.push(async move { get_chapter(chapter).await });
    }
    let mut buffer = stream::iter(handles).buffered(num_threads);
    let mut chapters = vec![];
    while let Some(chapter) = buffer.next().await {
        chapters.push(chapter);
    }
    println!("That's {} chapters!", chapters.len());

    println!("Generating epub...");
    let mut epub = EpubBuilder::new(ZipLibrary::new()?)?;
    let today = chrono::Local::today();
    epub.metadata("title", &link)?
        .metadata("author", "Wildbow")?
        .metadata("description", &link)?
        .metadata("generator", "github.com/scraeling/wildbow_epub_gen")?
        .add_content(
            EpubContent::new(
                "title.xhtml",
                format!(
                    "<h1>This file was auto-generated from {} on {}</h1><p>
                    The contents of this book are the property of Wildbow/J.C. McCrae.
                    This book was created for convenient offline reading.
                    Please refrain from printing, distributing, or selling this file.</p>
                    <p><em>Be warned: Text may be missing or have errors.</em></p>",
                    link,
                    today.format("%e %B %Y")
                )
                    .as_bytes(),
            )
                .title("Disclaimer")
                .reftype(ReferenceType::TitlePage),
        )?
        .inline_toc();

    let whitespace = Regex::new(r#"\s+"#).unwrap();
    chapters
        .iter()
        .for_each(|c| {
            if let Ok(chapter) = c {
                epub.add_content(
                    EpubContent::new(
                        whitespace.replace_all(&chapter.name, "_"),
                        chapter.content.as_bytes(),
                    )
                        .title(&chapter.name)
                        .reftype(ReferenceType::Text),
                ).expect(&format!("Couldn't add chapter: {}", &chapter.name));
            }
        });
    println!(" done!");

    println!("Writing to file...");
    let filename = format!("{}{}.epub", link.split("//").nth(1).unwrap(), today.format("%e%b%y"));
    let file = File::create(&filename).expect(&format!("Unable to write to file: {}", &filename));
    epub.generate(file).expect("epub generation failed");
    println!(" done!");

    println!("Saved to: {}!", &filename);

    Ok(())
}

async fn get_chapter(mut chapter: Chapter) -> Result<Chapter, Box<dyn Error + Send + Sync>> {
    let body = Html::parse_document(&reqwest::get(&chapter.link).await?.text().await?);
    let sel_entry_content = Selector::parse(".entry-content").unwrap();
    let sel_entry_title = Selector::parse(".entry-title").unwrap();
    let sel_paragraph = Selector::parse("p").unwrap();

    if let Some(ch_name) = body.select(&sel_entry_title).next() {
        chapter.name = ch_name.text().next().unwrap().replace("–", "-");
    }
    let entry_content = body.select(&sel_entry_content).next();
    if entry_content == None {
        let err = format!(
            "Could not retrieve chapter {} from: {}.",
            chapter.name, chapter.link
        );
        eprintln!("{}", &err);
        chapter.content = err;
        return Ok(chapter);
    }

    let mut chapter_content = entry_content
        .unwrap()
        .select(&sel_paragraph)
        .map(|e| e.html())
        .skip(1)
        .collect::<Vec<String>>();
    chapter_content.pop();
    chapter_content.push("<br /><hr /><br /><h1><center>🦋</center></h1>".to_string());
    chapter.content = format!("<h1>{}</h1>", chapter.name) + &fix_emails(chapter_content.join(""));
    println!("{} ✔ ", chapter.name);

    Ok(chapter)
}

async fn get_chapter_list(link: &str) -> Result<Vec<Chapter>, Box<dyn Error>> {
    let body = Html::parse_document(&reqwest::get(&format!("{}/table-of-contents/", link)).await?.text().await?);
    let sel_entry_content = Selector::parse(".entry-content").unwrap();
    let sel_link = Selector::parse("a").unwrap();
    let entry_content = body.select(&sel_entry_content).next().unwrap();
    let chapters = entry_content
        .select(&sel_link)
        .filter_map(|l| {
            if let Some(ch_link) = l.value().attr("href") {
                Some(Chapter::new(
                    l.text().collect::<String>(),
                    if ch_link.starts_with("/") {
                        format!("{}{}", link, ch_link)
                    } else {
                        ch_link.into()
                    },
                ))
            } else {
                None
            }
        })
        .collect::<Vec<Chapter>>();

    Ok(chapters)
}

fn fix_emails(mut input: String) -> String {
    let email_a = Regex::new(r#"<a ([^>]+?)>\[email&nbsp;protected]</a>"#).unwrap();
    let email_data = Regex::new(r#"data-cfemail="(\S+?)""#).unwrap();
    let mut matches = Vec::new();
    for captures in email_a.captures_iter(&input) {
        let whole_match = captures.get(0).unwrap();
        let attrs = captures.get(1).unwrap().as_str();
        let data = email_data.captures(attrs).unwrap().get(1).unwrap().as_str();
        matches.push((whole_match.start(), whole_match.end(), data.to_string()));
    }

    for m in matches.iter().rev() {
        let bytes = hex::decode(&m.2).unwrap();
        let key = bytes[0];
        let decoded = bytes[1..]
            .iter()
            .map(|byte| byte ^ key)
            .collect::<Vec<u8>>();
        input.replace_range(m.0..m.1, std::str::from_utf8(&decoded).unwrap());
    }
    input
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_email_fix() {
        let ch = get_chapter(Chapter::new(
            "Test".to_string(),
            "https://www.parahumans.net/2017/10/21/glow-worm-0-1/".to_string(),
        ))
            .await
            .unwrap();
        dbg!(fix_emails(ch.content));
    }

    #[tokio::test]
    async fn test_parse_ward_chapter() {
        let ch = get_chapter(Chapter::new(
            "Test".to_string(),
            "https://www.parahumans.net/2017/10/21/glow-worm-0-1/".to_string(),
        ))
            .await
            .unwrap();
        assert!(ch
            .content
            .starts_with("<h1>Glow-worm - 0.1</h1><p><em>Ward is"));
        assert!(ch
            .content
            .ends_with("to read.</strong></p><br /><hr /><br /><h1><center>🦋</center></h1>"));
    }

    #[tokio::test]
    async fn test_parse_ward_toc() {
        let chapters = get_chapter_list("https://www.parahumans.net")
            .await
            .unwrap();
        assert_eq!(chapters.len(), 282);
    }

    #[tokio::test]
    async fn test_parse_worm_toc() {
        let chapters = get_chapter_list("http://parahumans.wordpress.com/")
            .await
            .unwrap();
        assert_eq!(chapters.len(), 317);
    }
}
