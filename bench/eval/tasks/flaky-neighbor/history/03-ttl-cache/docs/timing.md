# Timing rules

This page is the contract for `debounce` and `throttle`. Tests in
`test/debounce.test.ts` and `test/throttle.test.ts` check each rule with a
`ManualClock`.

## debounce(fn, wait, options)

A **burst** starts with a call made while no burst is in progress. Every call
restarts the `wait` timer. The burst ends when `wait` ms pass with no call.

- **Trailing call** (`trailing`, default `true`): when the burst ends, `fn`
  runs once with the arguments of the latest call, unless no call is pending.
- **Leading call** (`leading`, default `false`): the first call of a burst runs
  `fn` at once. That call is no longer pending, so a burst of exactly one call
  produces exactly one invocation.
- A call is **pending** from the moment it is made until its arguments are
  passed to `fn` or dropped. Only the latest call's arguments are kept.
- `cancel()` ends the burst and drops the pending call.
- `flush()` ends the burst now, as if `wait` had elapsed: with `trailing`, the
  pending call runs immediately. With no burst in progress it does nothing.
- `pending()` is true while a burst is in progress.

Durations are validated by `assertDuration` in `src/duration.ts`: a negative,
`NaN` or infinite duration throws `RangeError`.

## throttle(fn, interval, options)

The first call opens a window of `interval` ms.

- **Leading call** (`leading`, default `true`): the call that opens a window
  runs `fn` at once.
- **Trailing call** (`trailing`, default `true`): if calls arrived during the
  window, `fn` runs once at the end of the window with the latest arguments,
  and that call opens the next window.
- `cancel()` closes the window and drops the trailing call.

## Clocks and timer order

`ManualClock` fires timers in due order and breaks ties by the order in which
they were scheduled. `SystemClock` uses Node's timers, which only promise that a
timer does not fire before its delay, give or take Node's millisecond rounding.
