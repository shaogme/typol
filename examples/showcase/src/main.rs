use std::process::ExitCode;

use typol::{TypolError, opeo::ErrSlot};
use typol_showcase::load_demo_config;

fn main() -> ExitCode {
    let mut load_errors = ErrSlot::<TypolError>::new();
    match load_errors.try_call(load_demo_config) {
        Ok(config) => {
            println!(
                "服务 {} 已加载，监听地址 {}:{}，线程范围 {}–{}，TLS={}，对象存储={}",
                config.service_name(),
                config.endpoint().host(),
                config.endpoint().port(),
                config.worker_pool().min_threads(),
                config.worker_pool().max_threads(),
                config.uses_tls(),
                config.uses_object_storage(),
            );
            ExitCode::SUCCESS
        }
        Err(caught) => {
            eprintln!("{}", caught.get());
            ExitCode::FAILURE
        }
    }
}
