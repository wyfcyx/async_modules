mod modules;

use modules::*;
use std::sync::Arc;
use std::thread;

async fn handler1(args: AsyncCallArguments, task_id: usize) -> AsyncCallReturnValue {
    println!("into handler1");
    let original_value: i32 = 100;
    let async_call_args = AsyncCallArguments {
        caller_module_id: 1,
        caller_task_id: task_id,
        callee_module_id: 2,
        data: [&original_value as *const _ as usize, 0, 0, 0, 0],
    };
    println!("handler1: before async call");
    let async_call_ret = async_call(async_call_args).await;
    println!("handler1: after async call");
    assert_eq!(original_value, 200);
    assert_eq!(async_call_ret.status, 233);
    AsyncCallReturnValue {
        caller_task_id: args.caller_task_id,
        status: 666,
    }
}

async fn handler2(args: AsyncCallArguments, _task_id: usize) -> AsyncCallReturnValue {
    let value = unsafe { &mut *(args.data[0] as *mut i32) };
    *value *= 2;
    AsyncCallReturnValue {
        caller_task_id: args.caller_task_id,
        status: 233,
    }
}

fn main() {
    let module1 = Arc::new(AsyncModuleImpl::new(1, Box::new(|(args, task_id)| Box::pin(handler1(args, task_id)))));
    let module2 = Arc::new(AsyncModuleImpl::new(2, Box::new(|(args, task_id)| Box::pin(handler2(args, task_id)))));
    MOD_MANAGER
        .write()
        .unwrap()
        .load_module(1, Arc::clone(&module1) as Arc<dyn AsyncModule>);
    MOD_MANAGER
        .write()
        .unwrap()
        .load_module(2, Arc::clone(&module2) as Arc<dyn AsyncModule>);

    module1.spawn_task(AsyncCallArguments {
        caller_module_id: 0,
        caller_task_id: 0,
        callee_module_id: 1,
        data: [0; 5],
    });

    let mod1_thread = thread::spawn(move || {
        module1.event_loop();
    });
    let mod2_thread = thread::spawn(move || {
        module2.event_loop();
    });

    mod1_thread.join().unwrap();
    mod2_thread.join().unwrap();
    println!("Hello world!");
}
