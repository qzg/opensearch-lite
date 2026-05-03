use std::collections::{BTreeMap, BTreeSet};

use opensearch_lite::api_spec::{inventory, AccessClass, Tier};

#[derive(Default)]
struct TierCounts {
    implemented: usize,
    best_effort: usize,
    mocked: usize,
    agent_read: usize,
    agent_write: usize,
    unsupported: usize,
    outside_identity: usize,
}

impl TierCounts {
    fn add(&mut self, tier: Tier) {
        match tier {
            Tier::Implemented => self.implemented += 1,
            Tier::BestEffort => self.best_effort += 1,
            Tier::Mocked => self.mocked += 1,
            Tier::AgentRead => self.agent_read += 1,
            Tier::AgentWrite => self.agent_write += 1,
            Tier::Unsupported => self.unsupported += 1,
            Tier::OutsideIdentity => self.outside_identity += 1,
        }
    }

    fn total(&self) -> usize {
        self.implemented
            + self.best_effort
            + self.mocked
            + self.agent_read
            + self.agent_write
            + self.unsupported
            + self.outside_identity
    }

    fn deterministic(&self) -> usize {
        self.implemented + self.best_effort + self.mocked
    }

    fn fallback(&self) -> usize {
        self.agent_read + self.agent_write
    }

    fn closed(&self) -> usize {
        self.unsupported + self.outside_identity
    }
}

fn main() {
    let routes = inventory();
    let total = routes.len();
    let unique_apis = routes
        .iter()
        .map(|route| route.name)
        .collect::<BTreeSet<_>>()
        .len();

    let mut tiers = TierCounts::default();
    let mut access_counts = BTreeMap::<&'static str, usize>::new();
    let mut families = BTreeMap::<String, TierCounts>::new();

    for route in routes {
        tiers.add(route.tier);
        *access_counts.entry(access_label(route.access)).or_default() += 1;
        families
            .entry(family_name(route.name).to_string())
            .or_default()
            .add(route.tier);
    }

    println!("# API Coverage Visualization");
    println!();
    println!(
        "Generated from `opensearch_lite::api_spec::inventory()` over the pinned OpenSearch REST spec plus OpenSearch Lite manual route overlays."
    );
    println!();
    println!("## Summary");
    println!();
    println!("| Metric | Count | Share |");
    println!("| --- | ---: | ---: |");
    println!("| Route shapes in inventory | {total} | 100.0% |");
    println!("| Unique API names | {unique_apis} | - |");
    println!(
        "| Deterministic local responses | {} | {} |",
        tiers.deterministic(),
        percent(tiers.deterministic(), total)
    );
    println!(
        "| Runtime fallback eligible | {} | {} |",
        tiers.fallback(),
        percent(tiers.fallback(), total)
    );
    println!(
        "| Closed or outside product identity | {} | {} |",
        tiers.closed(),
        percent(tiers.closed(), total)
    );
    println!();
    println!("Deterministic local responses combine `implemented`, `best_effort`, and `mocked` routes. Fallback-eligible routes still require runtime agent configuration and are not deterministic local parity.");
    println!();

    println!("## Coverage Funnel");
    println!();
    println!("```mermaid");
    println!("flowchart LR");
    println!("  all[\"OpenSearch route shapes\\n{total}\"]");
    println!(
        "  deterministic[\"Deterministic local\\n{} ({})\"]",
        tiers.deterministic(),
        percent(tiers.deterministic(), total)
    );
    println!(
        "  fallback[\"Fallback eligible\\n{} ({})\"]",
        tiers.fallback(),
        percent(tiers.fallback(), total)
    );
    println!(
        "  closed[\"Closed / unsupported\\n{} ({})\"]",
        tiers.closed(),
        percent(tiers.closed(), total)
    );
    println!("  all --> deterministic");
    println!("  all --> fallback");
    println!("  all --> closed");
    println!("```");
    println!();

    println!("## Tier Mix");
    println!();
    println!("```mermaid");
    println!("pie showData");
    println!("  \"Implemented\" : {}", tiers.implemented);
    println!("  \"Best effort\" : {}", tiers.best_effort);
    println!("  \"Mocked\" : {}", tiers.mocked);
    println!("  \"Agent read fallback\" : {}", tiers.agent_read);
    println!("  \"Agent write fallback\" : {}", tiers.agent_write);
    println!("  \"Unsupported\" : {}", tiers.unsupported);
    if tiers.outside_identity > 0 {
        println!("  \"Outside identity\" : {}", tiers.outside_identity);
    }
    println!("```");
    println!();

    println!("| Tier | Count | Share |");
    println!("| --- | ---: | ---: |");
    print_tier_row("Implemented", tiers.implemented, total);
    print_tier_row("Best effort", tiers.best_effort, total);
    print_tier_row("Mocked", tiers.mocked, total);
    print_tier_row("Agent read fallback", tiers.agent_read, total);
    print_tier_row("Agent write fallback", tiers.agent_write, total);
    print_tier_row("Unsupported", tiers.unsupported, total);
    if tiers.outside_identity > 0 {
        print_tier_row("Outside identity", tiers.outside_identity, total);
    }
    println!();

    println!("## Access Mix");
    println!();
    println!("```mermaid");
    println!("pie showData");
    for (label, count) in &access_counts {
        println!("  \"{label}\" : {count}");
    }
    println!("```");
    println!();

    println!("| Access class | Count | Share |");
    println!("| --- | ---: | ---: |");
    for (label, count) in &access_counts {
        println!("| {label} | {count} | {} |", percent(*count, total));
    }
    println!();

    let mut family_rows = families.into_iter().collect::<Vec<_>>();
    family_rows.sort_by(|(left_name, left), (right_name, right)| {
        right
            .deterministic()
            .cmp(&left.deterministic())
            .then_with(|| right.total().cmp(&left.total()))
            .then_with(|| left_name.cmp(right_name))
    });

    println!("## Family Coverage");
    println!();
    println!("```mermaid");
    println!("xychart-beta");
    println!("  title \"Top Families by Deterministic Route Shapes\"");
    let top = family_rows.iter().take(10).collect::<Vec<_>>();
    println!(
        "  x-axis [{}]",
        top.iter()
            .map(|(family, _)| format!("\"{}\"", mermaid_escape(family)))
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!(
        "  y-axis \"Routes\" 0 --> {}",
        top.iter()
            .map(|(_, counts)| counts.deterministic())
            .max()
            .unwrap_or(1)
    );
    println!(
        "  bar [{}]",
        top.iter()
            .map(|(_, counts)| counts.deterministic().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!("```");
    println!();

    println!("| Family | Total | Deterministic | Fallback eligible | Closed | Implemented | Best effort | Mocked |");
    println!("| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |");
    for (family, counts) in family_rows {
        println!(
            "| `{family}` | {} | {} | {} | {} | {} | {} | {} |",
            counts.total(),
            counts.deterministic(),
            counts.fallback(),
            counts.closed(),
            counts.implemented,
            counts.best_effort,
            counts.mocked
        );
    }
    println!();

    println!("## Refresh");
    println!();
    println!("```sh");
    println!("cargo run --example api_coverage > docs/api-coverage.md");
    println!("```");
}

fn access_label(access: AccessClass) -> &'static str {
    match access {
        AccessClass::Read => "Read",
        AccessClass::Write => "Write",
        AccessClass::Admin => "Admin",
    }
}

fn family_name(name: &str) -> &str {
    if let Some((family, _)) = name.split_once('.') {
        return family;
    }
    match name {
        "count" | "explain" | "mget" | "msearch" | "search" | "scroll" | "clear_scroll" => "search",
        "create" | "delete" | "exists" | "exists_source" | "get" | "get_source" | "index"
        | "update" => "documents",
        "delete_by_query"
        | "delete_by_query_rethrottle"
        | "reindex"
        | "reindex_rethrottle"
        | "update_by_query"
        | "update_by_query_rethrottle" => "by_query",
        "bulk" => "bulk",
        "info" | "ping" => "core",
        "put_script" | "get_script" | "delete_script" => "scripts",
        other => other,
    }
}

fn print_tier_row(label: &str, count: usize, total: usize) {
    println!("| {label} | {count} | {} |", percent(count, total));
}

fn percent(count: usize, total: usize) -> String {
    if total == 0 {
        return "0.0%".to_string();
    }
    format!("{:.1}%", (count as f64 / total as f64) * 100.0)
}

fn mermaid_escape(label: &str) -> String {
    label.replace('"', "\\\"")
}
