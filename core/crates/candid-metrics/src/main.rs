use chrono::{NaiveDate, TimeZone};
use chrono_tz::Tz;
use candid_metrics::{measure, timeline::Snapshot, Dataset, Result};
use std::{collections::HashMap, path::Path};
fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["--help"] {
        println!("candid-metrics (--fixture JSON | --timeline SQLITE) --start YYYY-MM-DD --end YYYY-MM-DD --timezone Asia/Singapore\n结束日不含在内；只输出数字与方法；时间线库只读，不启动服务或执行迁移。");
        return Ok(());
    }
    if args.len() % 2 != 0 {
        return Err("参数需要名称和值；使用 --help 查看用法".into());
    }
    let mut opts = HashMap::new();
    for pair in args.chunks(2) {
        if !["--fixture", "--timeline", "--start", "--end", "--timezone"]
            .contains(&pair[0].as_str())
            || opts.insert(pair[0].as_str(), pair[1].as_str()).is_some()
        {
            return Err("未知或重复参数".into());
        }
    }
    let get = |k| {
        opts.get(k)
            .copied()
            .ok_or("缺少必填参数；使用 --help 查看用法")
    };
    let start =
        NaiveDate::parse_from_str(get("--start")?, "%Y-%m-%d").map_err(|_| "起始日期无效")?;
    let end = NaiveDate::parse_from_str(get("--end")?, "%Y-%m-%d").map_err(|_| "结束日期无效")?;
    let tz: Tz = get("--timezone")?.parse().map_err(|_| "时区无效")?;
    if end <= start || (end - start).num_days() > 3660 {
        return Err("日期范围必须为 1–3660 天".into());
    }
    let report = match (opts.get("--fixture"), opts.get("--timeline")) {
        (Some(file), None) => {
            let bytes = std::fs::read(file).map_err(|_| "无法读取评估输入")?;
            // Do not echo malformed source values in parser errors.
            let data: Dataset = serde_json::from_slice(&bytes).map_err(|_| "评估输入格式无效")?;
            measure(&data, start, end, tz)?
        }
        (None, Some(file)) => {
            let ms = |d: NaiveDate| {
                tz.from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap())
                    .single()
                    .map(|t| t.timestamp_millis())
                    .ok_or("午夜无效")
            };
            Snapshot::read(Path::new(file), ms(start)?, ms(end)?)?.measure(start, end, tz)?
        }
        _ => return Err("必须选择一个 --fixture 或 --timeline 输入".into()),
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("指标测量失败：{error}。使用 --help 查看用法。");
        std::process::exit(2);
    }
}
