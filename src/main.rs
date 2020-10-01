use chrono;
use epub_builder::{EpubBuilder, EpubContent, ReferenceType, ZipLibrary};
use futures::{stream, StreamExt};
use reqwest;
use scraper::{Html, Selector};
use std::{env::args, error::Error, fs::File};
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
            link: if !link.starts_with("http") {format!("https://{}", link)} else {link},
            content: String::new(),
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let num_threads: usize = args().nth(2).unwrap_or("16".to_string()).parse()?;
    let link = args().nth(1).expect("No link provided");
    println!("Creating an ebook for: {}", &link);
    
    println!("Getting table of contents...");
    let chapters = get_chapter_list(&format!("{}/table-of-contents", &link)).await?;
    println!(" done!"); // TODO: Delayed. Might require flushing.

    println!("Downloading chapters... this might take a while.");

    let mut handles = Vec::with_capacity(chapters.len());
    for chapter in chapters {
        handles.push(async move {get_chapter(chapter).await});
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
    epub.metadata("title", &link)? // TODO: Figure out a way to get the title.
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
                ).as_bytes(),
            )
            .title(&link)
            .reftype(ReferenceType::TitlePage),
        )?
        .inline_toc();

    for chapter in chapters {
        let chapter = match chapter {
            Ok(chapter) => chapter,
            Err(_) => continue
        };
        epub.add_content(
            EpubContent::new(
                chapter.name.trim().replace(" ", "_"),
                chapter.content.as_bytes(),
            )
            .title(chapter.name.trim())
            .reftype(ReferenceType::Text),
        )?;
    }
    println!(" done!");

    println!("Writing to file...");
    let filename = format!(
        "{}{}.epub",
        link.split("//").nth(1).unwrap(),
        today.format("%e%b%y")
    );
    let file = File::create(&filename).expect(&format!("Unable to write to file: {}", &filename));
    epub.generate(file).expect("epub generation failed");
    println!(" done!");

    println!("Saved to: {}!", &filename);
    
    Ok(())
}

async fn get_chapter(mut chapter: Chapter) -> Result<Chapter, Box<dyn Error + Send + Sync>> {
    
    let body = Html::parse_document(&reqwest::get(&chapter.link).await?.text().await?);
    let sel_entry_content = Selector::parse(".entry-content").unwrap();
    let sel_paragraph = Selector::parse("p").unwrap();
    
    let entry_content = body.select(&sel_entry_content).next();
    if entry_content == None {
        let err = format!("Could not retrieve chapter {} from: {}.", chapter.name, chapter.link);
        eprintln!("{}", &err);
        chapter.content = err;
        return Ok(chapter)
    }
    
    let mut chapter_content = entry_content.unwrap()
        .select(&sel_paragraph)
        .map(|e| e.html())
        .skip(1)
        .collect::<Vec<String>>();
    chapter_content.pop();
    chapter_content.push("<br /><hr /><br /><h1><center>🦋</center></h1>".to_string());
    chapter.content = format!("<h1>{}</h1>", chapter.name) + &chapter_content.join("");
    println!("{} ✔ ", chapter.name.trim());
    
    Ok(chapter)
}

async fn get_chapter_list(link: &str) -> Result<Vec<Chapter>, Box<dyn Error>> {
    
    let body = Html::parse_document(&reqwest::get(link).await?.text().await?);
    let sel_entry_content = Selector::parse("div.entry-content").unwrap();
    let sel_link = Selector::parse("a").unwrap();
    let entry_content = body.select(&sel_entry_content).next().unwrap();
    let chapters = entry_content
        .select(&sel_link)
        .map(|l|
            Chapter::new(
                l.text().collect::<String>(),
                l.value().attr("href").unwrap().into(),
            )
        ).collect::<Vec<Chapter>>();
    
    Ok(chapters)
}

#[cfg(test)]
mod tests {
    
    use super::*;

    #[tokio::test]
    async fn test_parse_ward_chapter() {
        
        let ch = get_chapter(Chapter::new(
            "Test".to_string(),
            "https://www.parahumans.net/2017/10/21/glow-worm-0-1/".to_string(),
        )).await.unwrap();
        assert!(ch.content.starts_with("<h1>Test</h1><p><em>Ward is"));
        assert!(ch.content.ends_with("to read.</strong></p><br /><hr /><br /><h1><center>🦋</center></h1>"));
    }

    #[tokio::test]
    async fn test_parse_ward_toc() {
        
        assert_eq!(
            get_chapter_list("https://parahumans.net/table-of-contents").await
                .unwrap().len(),
            282
        );
    }
    #[tokio::test]
    async fn test_parse_worm_toc() {
        
        assert_eq!(
            get_chapter_list("https://parahumans.wordpress.com/table-of-contents").await
                .unwrap().len(),
            317
        );
    }
}
