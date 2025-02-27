# Crimes

Use crimes to write ergonomic state machines using futures.


## A note on API changes

After writing my [Socrates is a state machine][0] blog post I couldn't resist trying to simplify the API,
so rather than what is presented there (a paired runner and state machine) the updated API now handles
everything with a single struct: `PinnedStateMachine`:

```rust
fn read_9p_sync_from_bytes<T, R>(r: &mut R) -> io::Result<T>
where
    T: Read9p,
    R: Read,
{
    let mut state_machine = NinepParser::initialize(NinepState); // This is a PinnedStateMachine

    loop {
        match state_machine.step() {
            Step::Complete(res) => return res,
            Step::Pending(n) => {
                println!("{n} bytes requested");
                let mut buf = vec![0; n];
                r.read_exact(&mut buf)?;
                state_machine.send(buf);
            }
        }
    }
}
```

To take a look through the API as it was when I wrote the blog post you'll need to go [here][1].

  [0]: https://www.sminez.dev/socrates-is-a-state-machine/
  [1]: https://github.com/sminez/crimes/tree/1ea8a028f861b7d6061f3153af5532fc77856058
