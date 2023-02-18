use anyhow::{Context, Result};
use skim::prelude::*;

fn skim_select<I, T>(data: I, options: &SkimOptions) -> Result<Vec<T>>
where
    T: SkimItem + Clone,
    I: IntoIterator<Item = T>,
{
    let (tx_item, rx_item): (SkimItemSender, SkimItemReceiver) = unbounded();

    for item in data {
        let _ = tx_item.send(Arc::new(item));
    }

    drop(tx_item); // so that skim could know when to stop waiting for more items.

    let selected_items = Skim::run_with(options, Some(rx_item))
        .filter(|out| !out.is_abort)
        .map(|out| out.selected_items)
        .unwrap_or_else(Vec::new);

    let result: Vec<T> = selected_items
        .iter()
        .filter_map(|v| (**v).as_any().downcast_ref())
        .cloned()
        .collect::<Vec<T>>();

    Ok(result)
}

pub fn select_many<I, T>(data: I, query: Option<&str>) -> Result<Vec<T>>
where
    T: SkimItem + Clone,
    I: IntoIterator<Item = T>,
{
    let options = SkimOptionsBuilder::default()
        .height(Some("20%"))
        .query(query)
        .select1(query.is_some())
        .multi(true)
        .bind(vec!["ctrl-a:beginning-of-line", "ctrl-e:end-of-line"])
        .build()
        .unwrap();

    skim_select(data, &options).and_then(|arr| {
        if !arr.is_empty() {
            Ok(arr)
        } else {
            anyhow::bail!("No items selected")
        }
    })
}

pub fn select_one<I, T>(data: I, query: Option<&str>) -> Result<T>
where
    T: SkimItem + Clone,
    I: IntoIterator<Item = T>,
{
    let options = SkimOptionsBuilder::default()
        .height(Some("20%"))
        .query(query)
        .select1(query.is_some())
        .build()
        .unwrap();

    skim_select(data, &options)?
        .first()
        .cloned()
        .context("No item was selected")
}

pub fn format_datetime(datetime: &chrono::DateTime<chrono::FixedOffset>) -> String {
    let duration = chrono::Utc::now().signed_duration_since(*datetime);

    match (
        duration.num_hours(),
        duration.num_minutes(),
        duration.num_seconds(),
    ) {
        (12.., _, _) => datetime
            .with_timezone(&chrono::Local)
            .format("%a, %d %b %R")
            .to_string(),
        (hours @ 2..=12, _, _) => format!("{hours} hours ago"),
        (hours @ 1, _, _) => format!("{hours} hour ago"),
        (_, mins @ 2.., _) => format!("{mins} minutes ago"),
        (_, mins @ 1, _) => format!("{mins} minute ago"),
        (_, _, secs @ 10..) => format!("{secs} seconds ago"),
        (_, _, _) => "a few moments ago".to_string(),
    }
}
