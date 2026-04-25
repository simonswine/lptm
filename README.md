# lptm

**lptm** is a terminal UI for exploring Prometheus data through Grafana — the Grafana Explore experience, but in your terminal.

## Features

- Browse and filter Prometheus datasources configured in Grafana
- Write and execute PromQL instant queries with syntax highlighting
- Tab-completion for metric names, label names, and label values
- Results displayed as a structured table
- Query history scoped per datasource and Grafana instance, persisted across sessions

## Requirements

- A running Grafana instance with at least one Prometheus datasource
- A Grafana service account token with datasource read access

## Usage

```sh
lptm --grafana-url http://localhost:3000 --grafana-token <token>
```

Or via environment variables:

```sh
export GRAFANA_URL=http://localhost:3000
export GRAFANA_TOKEN=<token>
lptm
```

## Key bindings

### Datasource list

| Key | Action |
|-----|--------|
| `j` / `↓` | Move down |
| `k` / `↑` | Move up |
| Type | Filter datasources |
| `Enter` | Open query editor |
| `Esc` | Clear filter / quit |

### Query editor

| Key | Action |
|-----|--------|
| `Enter` | Execute query |
| `Tab` | Accept completion |
| `↑` / `↓` | Navigate history / completions |
| `←` / `→` | Move cursor |
| `Esc` | Back to datasource list |
| `Ctrl+C` | Quit |

## Configuration

lptm stores its data in `~/.config/lptm/`:

| File | Purpose |
|------|---------|
| `history` | Query history (JSONL, one entry per line) |
| `debug.log` | Debug log |

Each history entry records the query, datasource name and type, a hash of the Grafana URL, and a Unix timestamp — so history is automatically scoped when you switch between datasources or Grafana instances.
