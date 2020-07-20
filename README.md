# Rust repositories list

This repository contains a scraped list of all the public GitHub repos with source code
written in the [Rust programming language][rust]. The source code for the scraper is
also included.

Everything in this repository, unless otherwise specified, is released under
the MIT license.

[rust]: https://www.rust-lang.org

## Running the scraper

To run the scraper, execute the program with the `GITHUB_TOKEN` environment
variable (containing a valid GitHub API token -- no permissions are required)
and the data directory as the first argument:

```
$ GITHUB_TOKEN=foobar cargo run --release -- data
```

The scraper automatically saves its state to disk, so it can be interrupted and
it will resume where it left. This also allows incremental updates of the list.

## Using the data

The data is available in the `data/github.csv` file, in CSV format. That file
contains the GitHub GraphQL ID of the repository, its name, and whether it
contains a `Cargo.toml` and `Cargo.lock`.

All the repositories contained in the dataset are marked as using the language
by GitHub. Some results might be inaccurate for this reason.
