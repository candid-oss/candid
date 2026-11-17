#!/usr/bin/env python3
"""Measure an existing App process, without controlling UI or starting capture."""
import argparse
import datetime
import json
import pathlib
import subprocess
import tempfile
import time


def directory_bytes(path):
    if path is None or not path.exists():
        return 0
    total = 0
    for file in path.rglob('*'):
        if file.is_file() and not file.is_symlink():
            try:
                total += file.stat().st_size
            except FileNotFoundError:
                pass
    return total


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--pid', type=int, required=True)
    parser.add_argument('--seconds', type=float, default=60)
    parser.add_argument('--interval', type=float, default=2)
    parser.add_argument('--data-dir', type=pathlib.Path)
    parser.add_argument('--output', type=pathlib.Path, required=True)
    parser.add_argument('--label', default='unspecified')
    args = parser.parse_args()
    if args.seconds < 1 or args.interval < 0.2:
        parser.error('duration must be >=1s, interval >=0.2s')
    args.output.parent.mkdir(parents=True, exist_ok=True)
    before_bytes = directory_bytes(args.data_dir)
    samples, cpu = [], []
    with tempfile.TemporaryDirectory(prefix='candid-resource-') as temp:
        probe = pathlib.Path(temp) / 'probe'
        subprocess.run(['xcrun', 'clang', '-O2', str(pathlib.Path(__file__).with_name('resource_probe.c')), '-o', str(probe)], check=True, capture_output=True)
        calibration = json.loads(subprocess.check_output([str(probe), '--self-test'], text=True))
        begin = time.monotonic()
        deadline = begin + args.seconds
        with args.output.with_suffix('.jsonl').open('w') as trace:
            while True:
                value = json.loads(subprocess.check_output([str(probe), str(args.pid)], text=True))
                value['elapsed_seconds'] = time.monotonic() - begin
                if samples:
                    delta = value['elapsed_seconds'] - samples[-1]['elapsed_seconds']
                    value['cpu_percent_one_core'] = (value['cpu_ns'] - samples[-1]['cpu_ns']) / 1e9 / delta * 100
                    cpu.append(value['cpu_percent_one_core'])
                samples.append(value)
                trace.write(json.dumps(value) + '\n'); trace.flush()
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    break
                time.sleep(min(args.interval, remaining))
    duration = samples[-1]['elapsed_seconds'] - samples[0]['elapsed_seconds']
    cpu_average = (samples[-1]['cpu_ns'] - samples[0]['cpu_ns']) / 1e9 / duration * 100
    after_bytes = directory_bytes(args.data_dir)
    report = {
        'label': args.label, 'pid': args.pid,
        'measured_at_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
        'duration_seconds': duration, 'sample_count': len(samples),
        'cpu_denominator': 'one logical core = 100%; proc_pid_rusage Mach ticks converted with mach_timebase_info',
        'cpu_calibration': calibration,
        'cpu_average_percent': cpu_average,
        'cpu_sample_p95_percent': sorted(cpu)[min(len(cpu)-1, int(len(cpu)*0.95))] if cpu else 0,
        'footprint_peak_mib': max(s['footprint_bytes'] for s in samples) / 1024**2,
        'footprint_start_mib': samples[0]['footprint_bytes'] / 1024**2,
        'footprint_end_mib': samples[-1]['footprint_bytes'] / 1024**2,
        'disk_growth_bytes': after_bytes-before_bytes,
        'process_disk_write_delta_bytes': samples[-1]['disk_written_bytes']-samples[0]['disk_written_bytes'],
        'idle_wakeup_delta': samples[-1]['wakeups']-samples[0]['wakeups'],
        'cpu_energy_estimate_delta_joules': (samples[-1]['cpu_energy_nj']-samples[0]['cpu_energy_nj']) / 1e9,
        'cpu_energy_estimate_average_watts': (samples[-1]['cpu_energy_nj']-samples[0]['cpu_energy_nj']) / 1e9 / duration,
        'performance_core_energy_estimate_delta_joules': (samples[-1]['performance_core_energy_nj']-samples[0]['performance_core_energy_nj']) / 1e9,
        'extra_counter_deltas': {key: samples[-1][key]-samples[0][key] for key in
            ['disk_read_bytes', 'interrupt_wakeups', 'instructions', 'cycles', 'billed_energy_raw',
             'serviced_energy_raw', 'billed_system_time_raw', 'serviced_system_time_raw']},
        'limitations': ['Measures this process only; WindowServer/GPU and battery are not included.', 'A short sample is not an 8-hour stability result.', 'No UI or capture state is changed by this tool.'],
    }
    args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2)+'\n')
    print(json.dumps(report, ensure_ascii=False, indent=2))


if __name__ == '__main__':
    main()
