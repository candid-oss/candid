#!/usr/bin/env python3
"""Read-only power trace for a manually established capture/OCR condition.

No app is launched, stopped or configured. Supply a separately validated macmon
binary; CPU/GPU/ANE/RAM/chip/system power are whole-machine counters. Per-process
CPU energy is reported separately and must not be treated as whole-app power.
"""
import argparse
import datetime
import json
import pathlib
import statistics
import sqlite3
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--label', required=True)
    parser.add_argument('--seconds', type=float, default=60)
    parser.add_argument('--pid', type=int, action='append', default=[])
    parser.add_argument('--power-tool', type=pathlib.Path, required=True)
    parser.add_argument('--output', type=pathlib.Path, required=True)
    parser.add_argument('--timeline-db', type=pathlib.Path)
    parser.add_argument('--expected-app', help='Validate captured input; requires timeline-db')
    parser.add_argument('--expected-title-marker', default='')
    args = parser.parse_args()
    if not 5 <= args.seconds <= 3600:
        parser.error('seconds must be in 5...3600')
    if args.output.exists() or args.output.with_suffix('.power.jsonl').exists():
        parser.error('output must be new; previous evidence is not overwritten')
    if not args.power_tool.is_absolute() or not args.power_tool.is_file():
        parser.error('power-tool must be an absolute path to a validated executable')
    if args.expected_title_marker and not args.expected_app:
        parser.error('expected-title-marker requires expected-app')
    if args.expected_app and not args.timeline_db:
        parser.error('expected-app requires timeline-db')
    args.output.parent.mkdir(parents=True, exist_ok=True)
    power_path = args.output.with_suffix('.power.jsonl')
    process_path = args.output.with_suffix('.process.jsonl')
    samples = {pid: [] for pid in args.pid}
    with tempfile.TemporaryDirectory(prefix='candid-power-') as temp:
        probe = pathlib.Path(temp) / 'probe'
        source = pathlib.Path(__file__).with_name('resource_probe.c')
        subprocess.run(['xcrun', 'clang', '-O2', str(source), '-o', str(probe)],
                       check=True, capture_output=True)
        calibration = json.loads(subprocess.check_output([str(probe), '--self-test'], text=True))
        tool_version = subprocess.check_output([str(args.power_tool), '--version'], text=True).strip()
        with power_path.open('w') as power, process_path.open('w') as trace, args.output.with_suffix('.stderr.log').open('w') as errors:
            monitor = subprocess.Popen([str(args.power_tool), 'pipe', '-i', '1000'], stdout=power, stderr=errors)
            try:
                time.sleep(3)  # Counter subscription settles before the measured window.
                start = time.time()
                deadline = time.monotonic() + args.seconds
                while True:
                    for pid in args.pid:
                        result = subprocess.run([str(probe), str(pid)], capture_output=True, text=True)
                        if result.returncode:
                            trace.write(json.dumps({'pid': pid, 'at_ms': int(time.time() * 1000), 'unavailable': True}) + '\n')
                            continue
                        value = json.loads(result.stdout)
                        value.update(pid=pid, at_ms=int(time.time() * 1000))
                        samples[pid].append(value)
                        trace.write(json.dumps(value) + '\n')
                    trace.flush()
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        break
                    time.sleep(min(1, remaining))
                end = time.time()
            finally:
                monitor.terminate()
                monitor.wait(timeout=10)
    rows = [json.loads(line) for line in power_path.read_text().splitlines()]
    rows = [row for row in rows if start <= datetime.datetime.fromisoformat(row['timestamp']).timestamp() <= end]
    if len(rows) < 2:
        raise RuntimeError('power samples unavailable; do not interpret missing counters as zero')
    keys = ['cpu_power', 'gpu_power', 'ane_power', 'ram_power', 'dcs_power', 'chip_power',
            'display_power', 'sys_power', 'dc_in_power']
    powers = {}
    for key in keys:
        values = sorted(row[key] for row in rows if isinstance(row.get(key), (int, float)))
        powers[key] = None if not values else {'mean_watts': statistics.mean(values),
            'median_watts': statistics.median(values), 'p95_watts': values[min(len(values) - 1, int(len(values) * .95))]}
    processes = {}
    for pid, values in samples.items():
        if len(values) < 2:
            processes[str(pid)] = {'unavailable': True}
            continue
        a, b = values[0], values[-1]
        seconds = (b['at_ms'] - a['at_ms']) / 1000
        delta = {key: b[key] - a[key] for key in a if key not in ['pid', 'at_ms']}
        processes[str(pid)] = {'duration_seconds': seconds, 'cpu_percent_one_core': delta['cpu_ns'] / 1e9 / seconds * 100,
                              'cpu_energy_estimate_joules': delta['cpu_energy_nj'] / 1e9,
                              'counter_deltas': delta, 'footprint_peak_mib': max(v['footprint_bytes'] for v in values) / 1024**2}
    report = {'label': args.label, 'start_ms': int(start * 1000), 'end_ms': int(end * 1000),
              'duration_seconds': end - start, 'power_sample_count': len(rows), 'power_tool': tool_version,
              'cpu_calibration': calibration, 'powers': powers, 'processes': processes,
              'limitations': ['Power domains cover the entire machine and include unrelated apps.',
                              'Component counters overlap; never sum them into total power.',
                              'billed/serviced raw counters have no joule conversion in this tool.',
                              'No capture, OCR, model, display or system settings were changed by this tool.']}
    if args.timeline_db:
        db = sqlite3.connect(args.timeline_db.resolve().as_uri() + '?mode=ro', uri=True)
        captured = db.execute("SELECT kind,app_bundle_id,COUNT(*),SUM(CASE WHEN title LIKE ? THEN 1 ELSE 0 END) "
                              "FROM events WHERE kind IN ('screen.shot','screen.text') AND start_ms BETWEEN ? AND ? "
                              "GROUP BY kind,app_bundle_id", ('%' + args.expected_title_marker + '%', report['start_ms'], report['end_ms'])).fetchall()
        report['captured_input_counts'] = captured
        if args.expected_app:
            report['expected_input_valid'] = bool(captured) and all(
                app == args.expected_app and count == matching for _, app, count, matching in captured)
        db.close()
    args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
    print(json.dumps({'label': args.label, 'powers': powers, 'processes': processes}, ensure_ascii=False))
    if report.get('expected_input_valid') is False:
        raise SystemExit('Captured input differed from the expected condition; report saved, comparison rejected.')


if __name__ == '__main__':
    main()
