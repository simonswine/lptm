# lptm

**lptm** is a terminal UI for exploring logs, profiles, traces, and metrics through Grafana — the Grafana Explore experience, but in your terminal.

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

### Universal

| Key | Action |
|-----|--------|
| `Ctrl+C` | Quit |
| `Ctrl+R` | Reload / re-execute current view |
| `Ctrl+T` | Open time range picker |

### Datasource list

| Key | Action |
|-----|--------|
| `j` / `↓` | Move down |
| `k` / `↑` | Move up |
| `/` | Open filter |
| `f` | Toggle favourite |
| `Tab` | Toggle history panel |
| `Enter` | Open query editor |
| `Esc` | Clear filter / quit |

### Query editor (Prometheus)

| Key | Action |
|-----|--------|
| `Enter` | Execute query |
| `Tab` | Accept completion |
| `↑` / `↓` | Navigate history / completions |
| `←` / `→` | Move cursor |
| `Home` / `Ctrl+A` | Jump to line start |
| `End` / `Ctrl+E` | Jump to line end |
| `Esc` | Back to datasource list |

### Pyroscope

| Key | Action |
|-----|--------|
| `j` / `↓` | Next service |
| `k` / `↑` | Previous service |
| `/` | Filter services |
| `p` | Open profile type selector |
| `Enter` | Load flamegraph |
| `Tab` | Cycle views (Flamegraph → Timeline → Heatmap → SpanHeatmap) |
| `h` / `←` | Move left in flamegraph |
| `l` / `→` | Move right in flamegraph |
| `k` / `↑` | Zoom out in flamegraph |
| `j` / `↓` | Zoom in in flamegraph |
| `Enter` / `z` | Zoom into frame |
| `Backspace` / `o` | Zoom out frame |
| `s` | Sandwich view |
| `q` / `Esc` | Back to service list |

### Tempo

| Key | Action |
|-----|--------|
| `Enter` | Execute query |
| `j` / `↓` | Next result |
| `k` / `↑` | Previous result |
| `←` / `→` | Move cursor |
| `Ctrl+A` | Jump to line start |
| `Ctrl+E` | Jump to line end |
| `Esc` | Back to datasource list |

### Tempo trace detail

| Key | Action |
|-----|--------|
| `j` / `↓` | Next span |
| `k` / `↑` | Previous span |
| `J` | Scroll attributes down |
| `K` | Scroll attributes up |
| `/` | Filter spans |
| `Esc` | Back to trace list |

### Loki

| Key | Action |
|-----|--------|
| `Enter` | Execute query |
| `Tab` | Accept completion |
| `↑` / `↓` | Navigate completions / results |
| `Ctrl+A` | Jump to line start |
| `Ctrl+E` | Jump to line end |
| `Esc` | Dismiss completions / back |

## Configuration

lptm stores its data in `~/.config/lptm/`:

| File | Purpose |
|------|---------|
| `history` | Query history (JSONL, one entry per line) |
| `debug.log` | Debug log |

Each history entry records the query, datasource name and type, a hash of the Grafana URL, and a Unix timestamp — so history is automatically scoped when you switch between datasources or Grafana instances.
