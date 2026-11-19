# Test data

The BSUP files here were written by SuperDB's `super` command so the tests
check that this crate reads what SuperDB writes and writes what SuperDB
reads.  To regenerate them with `super` on your path:

```sh
super -o types.cols.bsup types.sup
super -rows -o types.rows.bsup types.sup
super -s types.sup > types.expected.sup

python3 gen_mixed.py 600 > mixed.sup
super -o mixed.cols.bsup mixed.sup
super -rows -o mixed.rows.bsup mixed.sup
super -s mixed.sup > mixed.expected.sup
rm mixed.sup
```

`types.sup` covers each type of value, and `mixed.sup` (from the seeded
generator) has enough values of varied types to exercise the encodings of
column frames: bit-packed and variable-byte integer segments, dictionary and
constant vectors, LZ4-compressed segments, unions, options, and frames whose
values have more than one type.
