"""Summarizes a results directory into what the report shows.

    python3 summarize.py [RESULTS_DIR]        (default: results/latest)

Writes into RESULTS_DIR:
  summary.md     medians [min-max] per experiment and p50/p95 latencies
  pgfplots.tex   the coordinates of the two report charts (paste into main.tex)
  perf-size.png, perf-conc.png   the same charts, if matplotlib is installed
"""
import csv
import statistics as st
import sys
from collections import OrderedDict
from pathlib import Path

UP, DOWN = "#2563EB", "#D97706"  # report colors: uploads blue, downloads amber

out_dir = Path(sys.argv[1] if len(sys.argv) > 1 else Path(__file__).parent / "results" / "latest")


def groups(name):
    """label -> list of rows of perf-<name>.csv, in file order."""
    path = out_dir / f"perf-{name}.csv"
    if not path.exists():
        return OrderedDict()
    g = OrderedDict()
    for row in csv.DictReader(path.open()):
        if row["up_mibs"]:
            g.setdefault(row["label"], []).append(row)
    return g


def spread(values):
    return st.median(values), min(values), max(values)


def fmt(values):
    m, lo, hi = spread(values)
    return f"{m:.1f} [{lo:.1f}-{hi:.1f}]"


def percentile(values, p):
    v = sorted(values)
    return v[min(len(v) - 1, round(p * (len(v) - 1)))]


md = [f"# Resumen de {out_dir.name}", ""]
for name, title in [("size", "Tamaño de archivo"), ("conc", "Clientes concurrentes (total)"),
                    ("bsize", "Tamaño de bloque (256 MiB)"), ("repl", "R = 1 contra R = 2")]:
    g = groups(name)
    if not g:
        continue
    md += [f"## {title} (MiB/s, mediana [mín-máx])", "", "| caso | n | subida | descarga | verificación |", "|---|---|---|---|---|"]
    for label, rows in g.items():
        checks = sorted({r["check"] for r in rows})
        md.append(f"| {label} | {len(rows)} | {fmt([float(r['up_mibs']) for r in rows])} "
                  f"| {fmt([float(r['down_mibs']) for r in rows])} | {', '.join(checks)} |")
    md.append("")

lat = sorted(out_dir.glob("latency-*.txt"))
if lat:
    md += ["## Latencia (ms)", "", "| operación | n | p50 | p95 | máx |", "|---|---|---|---|---|"]
    for f in lat:
        v = [float(x) for x in f.read_text().split()]
        md.append(f"| {f.stem.removeprefix('latency-')} | {len(v)} | {percentile(v, .5):.1f} "
                  f"| {percentile(v, .95):.1f} | {max(v):.1f} |")
    md.append("")
(out_dir / "summary.md").write_text("\n".join(md) + "\n")


# pgfplots coordinates with asymmetric error bars (min/max around the median)
def coords(rows_by_x, key):
    parts = []
    for x, rows in rows_by_x:
        m, lo, hi = spread([float(r[key]) for r in rows])
        parts.append(f"({x},{m:.1f}) += (0,{hi - m:.1f}) -= (0,{m - lo:.1f})")
    return "coordinates {" + "\n               ".join(parts) + "};"


tex = []
size = groups("size")
if size:
    xs = [({"f16m": "16 MiB", "f100m": "100 MiB", "f1g": "1 GiB"}[k], v) for k, v in size.items()]
    tex += ["% Figura perf-tamano", "% subida", coords(xs, "up_mibs"), "% descarga", coords(xs, "down_mibs"), ""]
conc = groups("conc")
if conc:
    xs = [(k.removeprefix("c"), v) for k, v in conc.items()]
    tex += ["% Figura perf-concurrencia", "% subida", coords(xs, "up_mibs"), "% descarga", coords(xs, "down_mibs"), ""]
(out_dir / "pgfplots.tex").write_text("\n".join(tex) + "\n")

try:
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
except ImportError:
    print(f"wrote {out_dir}/summary.md and pgfplots.tex (install matplotlib for PNG charts)")
    sys.exit(0)


def errbars(rows_list, key):
    s = [spread([float(r[key]) for r in rows]) for rows in rows_list]
    med = [m for m, _, _ in s]
    return med, [[m - lo for m, lo, _ in s], [hi - m for m, _, hi in s]]


def style(ax, ylabel, xlabel):
    ax.set_ylabel(ylabel)
    ax.set_xlabel(xlabel)
    ax.grid(axis="y", color="#e5e7eb", linewidth=0.8)
    ax.set_axisbelow(True)
    for side in ("top", "right"):
        ax.spines[side].set_visible(False)
    ax.legend(frameon=False, ncol=2, loc="upper center", bbox_to_anchor=(0.5, 1.12))


if size:
    labels = [{"f16m": "16 MiB", "f100m": "100 MiB", "f1g": "1 GiB"}[k] for k in size]
    fig, ax = plt.subplots(figsize=(6.4, 3.4))
    x = range(len(labels))
    for off, key, color, name in [(-0.18, "up_mibs", UP, "Subida"), (0.18, "down_mibs", DOWN, "Descarga")]:
        med, err = errbars(list(size.values()), key)
        ax.bar([i + off for i in x], med, width=0.34, color=color, label=name,
               yerr=err, error_kw={"ecolor": "#64748B", "elinewidth": 1, "capsize": 3})
    ax.set_xticks(list(x), labels)
    style(ax, "Throughput (MiB/s)", "Tamaño del archivo")
    fig.tight_layout()
    fig.savefig(out_dir / "perf-size.png", dpi=200)

if conc:
    xs = [int(k.removeprefix("c")) for k in conc]
    fig, ax = plt.subplots(figsize=(6.4, 3.4))
    for key, color, marker, name in [("up_mibs", UP, "o", "Subida"), ("down_mibs", DOWN, "s", "Descarga")]:
        med, err = errbars(list(conc.values()), key)
        ax.errorbar(xs, med, yerr=err, color=color, marker=marker, linewidth=2, markersize=6,
                    capsize=3, label=name)
    ax.set_xticks(xs)
    ax.set_ylim(bottom=0)
    style(ax, "Throughput total (MiB/s)", "Clientes concurrentes")
    fig.tight_layout()
    fig.savefig(out_dir / "perf-conc.png", dpi=200)

print(f"wrote {out_dir}/summary.md, pgfplots.tex and the PNG charts")
