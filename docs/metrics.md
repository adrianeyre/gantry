# Prometheus metrics

Gantry can expose [Prometheus](https://prometheus.io/) metrics at `/metrics`, in the text exposition format (PRD §7.7, §9.2). They are **off by default**.

## Turning them on

Choose one of these in `gantry.toml` (or the environment variable beside each one):

| Setting | Variable | What it does |
|---|---|---|
| `metrics_token` | `GANTRY_METRICS_TOKEN` | Serves `/metrics` beside the dashboard, but only to requests that bring `Authorization: Bearer <token>`. Anything else gets `401`. The token must be at least 16 characters long (try `openssl rand -hex 24`). |
| `metrics_bind` | `GANTRY_METRICS_BIND` | Serves `/metrics` on an address of its own, such as `127.0.0.1:9187` or a port only your monitoring network can reach. `/metrics` is then **not** served beside the dashboard at all. If `metrics_token` is also set, scrapes on this address need the token too. |

With neither set, `/metrics` is not served: the request falls through to the login-protected dashboard.

Behind a tunnel, prefer `metrics_bind` on an address the tunnel doesn't forward. A token-protected `/metrics` beside the dashboard is reachable wherever the dashboard is.

An example Prometheus scrape job using the token:

```yaml
scrape_configs:
  - job_name: gantry
    metrics_path: /metrics
    authorization:
      type: Bearer
      credentials: "<your metrics_token>"
    static_configs:
      - targets: ["gantry.local:8080"]
```

## Metrics

Names follow the Prometheus conventions: every metric starts with `gantry_`, values are in base units (seconds, degrees Celsius), counters end in `_total`, and 0-to-1 values end in `_ratio`. Per-Printer metrics carry the labels `printer` (Gantry's stable id) and `name` (the name it is shown under, which can change).

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `gantry_build_info` | gauge | `version` | Always 1. The running Gantry version. |
| `gantry_printers` | gauge | | How many Printers the Fleet has. |
| `gantry_printer_connected` | gauge | `printer`, `name`, `driver` | 1 when Gantry can talk to the Printer (Connection connected), 0 when it is connecting or disconnected. |
| `gantry_printer_state` | gauge | `printer`, `name`, `state` | 1 for the Printer State the Printer is in, 0 for the others. `state` is one of `idle`, `preparing`, `printing`, `paused`, `awaiting_clear` or `busy`. Every state is 0 while the Printer is disconnected, because its Printer State is then unknown. |
| `gantry_printer_temperature_celsius` | gauge | `printer`, `name`, `sensor` | A temperature the Printer reports. `sensor` is `nozzle`, `bed` or `chamber`; a sensor the Printer doesn't have is absent. |
| `gantry_printer_target_temperature_celsius` | gauge | `printer`, `name`, `sensor` | A heater's target temperature, while one is set. |
| `gantry_printer_job_progress_ratio` | gauge | `printer`, `name` | How far the running Job has got, 0 to 1. Absent when nothing is running. |
| `gantry_printer_job_remaining_seconds` | gauge | `printer`, `name` | How long the running Job is expected to take still, when the Printer says. |
| `gantry_jobs_total` | counter | `printer`, `name`, `outcome` | Jobs that have ended on each Printer, by Job Outcome: `succeeded`, `failed`, `cancelled` or `lost`. Counted from Job History, so it survives restarts. A Succeeded Outcome overruled at Mark Clear counts as `failed`. A Printer that has been removed keeps its counts, with an empty `name`. |
| `gantry_fleet_jobs_total` | counter | `outcome` | The same across the whole Fleet. Every Outcome is always present, so a rate is never missing. |
| `gantry_queue_jobs` | gauge | | Jobs waiting in the Queue. |

Some example queries:

- Printers that have dropped off: `gantry_printer_connected == 0`
- Printers waiting for someone to clear the bed: `gantry_printer_state{state="awaiting_clear"} == 1`
- The Fleet's failure rate over a week: `increase(gantry_fleet_jobs_total{outcome="failed"}[7d]) / ignoring(outcome) sum without(outcome) (increase(gantry_fleet_jobs_total[7d]))`
