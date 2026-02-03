---
name: snipmind-arch
description: SnipMind project architecture. Directory structure, components, state management, DB design, Phase 1 MVP.
---

# SnipMind Architecture Guide

## Overview

**SnipMind**: Code snippet manager as a "second brain for dev knowledge"

- **Stack**: Dioxus 0.7.x + SQLite (rusqlite)
- **Platform**: Linux (Phase 1), Windows/Android (Phase 2+)

## Directory Structure

```
snipmind/
├── src/
│   ├── main.rs              # Entry point
│   ├── app.rs               # Root component
│   ├── components/          # UI components
│   │   ├── sidebar.rs       # Tag list
│   │   ├── entry_list.rs    # Entry list
│   │   ├── entry_detail.rs  # Entry view
│   │   ├── entry_editor.rs  # Entry form
│   │   ├── search_bar.rs
│   │   └── tag_input.rs
│   ├── db/
│   │   ├── connection.rs    # DB connection
│   │   └── migrations.rs
│   ├── repositories/
│   │   ├── entry_repo.rs    # Entry CRUD
│   │   ├── tag_repo.rs
│   │   └── search_repo.rs   # FTS5 search
│   ├── models/
│   │   ├── entry.rs         # Entry, EntryVersion
│   │   └── tag.rs
│   └── state/
│       └── app_state.rs     # Global state
├── assets/main.css
├── migrations/001_initial.sql
└── Dioxus.toml
```

## Data Models

```rust
pub struct Entry {
    pub id: i64, pub title: String, pub versioned: bool,
    pub created_at: String, pub updated_at: String,
}

pub struct EntryVersion {
    pub id: i64, pub entry_id: i64, pub version: Option<String>,
    pub code: Option<String>, pub memo: Option<String>,
    pub language: String, pub reference_url: Option<String>,
}

pub struct EntryWithVersions {
    pub entry: Entry, pub versions: Vec<EntryVersion>, pub tags: Vec<Tag>,
}

pub struct Tag { pub id: i64, pub name: String }
pub struct TagWithCount { pub tag: Tag, pub entry_count: i64 }
```

## State Management

```rust
#[derive(Clone)]
pub struct AppState {
    pub entries: Signal<Vec<EntryListItem>>,
    pub selected_entry_id: Signal<Option<i64>>,
    pub selected_entry: Signal<Option<EntryWithVersions>>,
    pub search_query: Signal<String>,
    pub selected_tag: Signal<Option<String>>,
    pub is_editing: Signal<bool>,
    pub is_loading: Signal<bool>,
    pub tags: Signal<Vec<TagWithCount>>,
}

// Provider
#[component]
fn App() -> Element {
    use_context_provider(|| AppState::new());
    rsx! { /* ... */ }
}

// Consumer
let state = use_context::<AppState>();
```

## UI Layout

```
┌──────────────────────────────────────────────────────┐
│ [🔍 Search...                    ] [+ New] [⚙️]      │
├──────────────┬───────────────────────────────────────┤
│ Sidebar      │  Main Content                         │
│ (250px)      │  ┌─────────────────────────────────┐  │
│              │  │ # Title                         │  │
│ 📁 All (42)  │  │ Tags: tag1, tag2               │  │
│              │  │ [v0.8] [v0.7] ← Version tabs   │  │
│ ─── Tags ─── │  │ ```rust                        │  │
│ 📌 axum (8)  │  │ // code                        │  │
│ 📌 leptos    │  │ ```                            │  │
│              │  │ ## Memo                        │  │
│ ─── Recent ──│  └─────────────────────────────────┘  │
│ ...          │                                       │
├──────────────┴───────────────────────────────────────┤
│ [Edit] [Copy] [Delete]                Updated: 2m ago│
└──────────────────────────────────────────────────────┘
```

## Component Tree

```
App
├── SearchBar
├── Sidebar
│   ├── TagList → TagItem
│   └── RecentEntries → EntryListItem
└── MainContent
    ├── EntryDetail (view mode)
    │   ├── VersionTabs
    │   ├── CodeBlock
    │   └── MemoDisplay
    └── EntryEditor (edit mode)
```

## Repository Pattern

```rust
// list_entries(conn, tag, limit, offset) -> Vec<EntryListItem>
// get_entry(conn, id) -> EntryWithVersions
// create_entry(conn, entry, version) -> i64
// update_entry(conn, id, entry, version)
// delete_entry(conn, id)  // CASCADE
```

## Phase 1 Checklist

1. **Foundation**: main.rs, db/connection.rs, migrations.rs, 001_initial.sql
2. **Models**: entry.rs, tag.rs
3. **Repositories**: entry_repo.rs, tag_repo.rs, search_repo.rs
4. **State**: app_state.rs
5. **UI**: app.rs, sidebar.rs, entry_list.rs, entry_detail.rs, entry_editor.rs
6. **Style**: main.css

## Color Palette (Dark Theme)

```css
:root {
    --bg-primary: #1e1e2e;      /* Background */
    --bg-secondary: #313244;    /* Sidebar, cards */
    --text-primary: #cdd6f4;    /* Main text */
    --text-secondary: #a6adc8;  /* Sub text */
    --accent: #89b4fa;          /* Accent */
    --code-bg: #181825;         /* Code block */
}
```

## Related Skills

- `dioxus-guide` - Dioxus 0.7.x
- `rusqlite-guide` - SQLite operations
