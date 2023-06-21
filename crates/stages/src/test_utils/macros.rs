macro_rules! stage_test_suite {
    ($runner:ident, $name:ident) => {

         paste::item! {
            /// Check that the execution is short-circuited if the database is empty.
            /// 检查execution被短路，如果数据库是空的
            #[tokio::test]
            async fn [< execute_empty_db_ $name>] () {
                // Set up the runner
                let runner = $runner::default();

                // Execute the stage with empty database
                // 用空的数据库执行
                let input = crate::stage::ExecInput::default();

                // Run stage execution
                let result = runner.execute(input).await;
                // Check that the result is returned and the stage does not panic.
                // The return result with empty db is stage-specific.
                // 检查结果被返回，stage没有panic，返回结果是stage-specific
                assert_matches::assert_matches!(result, Ok(_));

                // Validate the stage execution
                // 校验stage的执行
                assert_matches::assert_matches!(
                    runner.validate_execution(input, result.unwrap().ok()),
                    Ok(_),
                    "execution validation"
                );
            }

            // Run the complete stage execution flow.
            // 运行完整的stage execution流程
            #[tokio::test]
            async fn [< execute_ $name>] () {
                let (target, current_checkpoint) = (500, 100);

                // Set up the runner
                let mut runner = $runner::default();
                let input = crate::stage::ExecInput {
                    target: Some(target),
                    checkpoint: Some(reth_primitives::stage::StageCheckpoint::new(current_checkpoint)),
                };
                let seed = runner.seed_execution(input).expect("failed to seed");
                let rx = runner.execute(input);

                // Run `after_execution` hook
                runner.after_execution(seed).await.expect("failed to run after execution hook");

                // Assert the successful result
                // 断言成功的结果
                let result = rx.await.unwrap();
                assert_matches::assert_matches!(
                    result,
                    Ok(ExecOutput { done, checkpoint })
                        if done && checkpoint.block_number == target
                );

                // Validate the stage execution
                // 校验stage execution
                assert_matches::assert_matches!(
                    runner.validate_execution(input, result.ok()),
                    Ok(_),
                    "execution validation"
                );
            }

            // Check that unwind does not panic on no new entries within the input range.
            // 检查unwind不会在input range内没有新的entries时panic
            #[tokio::test]
            async fn [< unwind_no_new_entries_ $name>] () {
                // Set up the runner
                let mut runner = $runner::default();
                let input = crate::stage::UnwindInput::default();

                // Seed the database
                runner.seed_execution(crate::stage::ExecInput::default()).expect("failed to seed");

                runner.before_unwind(input).expect("failed to execute before_unwind hook");

                // Run stage unwind
                // 运行stage unwind
                let rx = runner.unwind(input).await;
                assert_matches::assert_matches!(
                    rx,
                    Ok(UnwindOutput { checkpoint }) if checkpoint.block_number == input.unwind_to
                );

                // Validate the stage unwind
                // 校验stage unwind
                assert_matches::assert_matches!(
                    runner.validate_unwind(input),
                    Ok(_),
                    "unwind validation"
                );
            }

            // Run complete execute and unwind flow.
            // 运行完整的execute和unwind流程
            #[tokio::test]
            async fn [< unwind_ $name>] () {
                let (target, current_checkpoint) = (500, 100);

                // Set up the runner
                // 构建runner
                let mut runner = $runner::default();
                let execute_input = crate::stage::ExecInput {
                    target: Some(target),
                    checkpoint: Some(reth_primitives::stage::StageCheckpoint::new(current_checkpoint)),
                };
                let seed = runner.seed_execution(execute_input).expect("failed to seed");

                // Run stage execution
                // 运行stage execution
                let rx = runner.execute(execute_input);
                runner.after_execution(seed).await.expect("failed to run after execution hook");

                // Assert the successful execution result
                // 插入成功的执行结果
                let result = rx.await.unwrap();
                assert_matches::assert_matches!(
                    result,
                    Ok(ExecOutput { done, checkpoint })
                        if done && checkpoint.block_number == target
                );
                assert_matches::assert_matches!(
                    runner.validate_execution(execute_input, result.ok()),
                    Ok(_),
                    "execution validation"
                );


                // Run stage unwind
                // 运行stage unwind
                let unwind_input = crate::stage::UnwindInput {
                    unwind_to: current_checkpoint,
                    checkpoint: reth_primitives::stage::StageCheckpoint::new(target),
                    bad_block: None,
                };

                runner.before_unwind(unwind_input).expect("Failed to unwind state");

                let rx = runner.unwind(unwind_input).await;
                // Assert the successful unwind result
                assert_matches::assert_matches!(
                    rx,
                    Ok(UnwindOutput { checkpoint }) if checkpoint.block_number == unwind_input.unwind_to
                );

                // Validate the stage unwind
                assert_matches::assert_matches!(
                    runner.validate_unwind(unwind_input),
                    Ok(_),
                    "unwind validation"
                );
            }
        }
    };
}

// `execute_already_reached_target` is not suitable for the headers stage thus
// included in the test suite extension
macro_rules! stage_test_suite_ext {
    ($runner:ident, $name:ident) => {
        crate::test_utils::stage_test_suite!($runner, $name);

        paste::item! {
            /// Check that the execution is short-circuited if the target was already reached.
            /// 检查如果目标已经被到达，执行是否被短路
            #[tokio::test]
            async fn [< execute_already_reached_target_ $name>] () {
                let current_checkpoint = 1000;

                // Set up the runner
                // 构建runner
                let mut runner = $runner::default();
                let input = crate::stage::ExecInput {
                    target: Some(current_checkpoint),
                    checkpoint: Some(reth_primitives::stage::StageCheckpoint::new(current_checkpoint)),
                };
                let seed = runner.seed_execution(input).expect("failed to seed");

                // Run stage execution
                // 运行stage execution
                let rx = runner.execute(input);

                // Run `after_execution` hook
                // 运行`after_execution` hook
                runner.after_execution(seed).await.expect("failed to run after execution hook");

                // Assert the successful result
                // 断言成功的结果
                let result = rx.await.unwrap();
                assert_matches::assert_matches!(
                    result,
                    Ok(ExecOutput { done, checkpoint })
                        if done && checkpoint.block_number == current_checkpoint
                );

                // Validate the stage execution
                // 校验stage execution
                assert_matches::assert_matches!(
                    runner.validate_execution(input, result.ok()),
                    Ok(_),
                    "execution validation"
                );
            }
        }
    };
}

pub(crate) use stage_test_suite;
pub(crate) use stage_test_suite_ext;
