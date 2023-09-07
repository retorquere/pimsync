#!/bin/sh

cargo build -p live_tests || exit

for profile in live_tests/*.profile; do
  ./target/debug/live_tests "$profile"
done
