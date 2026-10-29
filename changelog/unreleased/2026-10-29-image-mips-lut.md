- **Decoding a large image no longer needs a full-size float copy.** `ImageCache` builds mip 0 by
  copying opaque pixels straight through and premultiplying the rest with sRGB lookup tables, and
  builds each smaller mip from the 8-bit level above it (mip 1 from the source), so the only memory
  it holds is the mips themselves. An 8192×8192 image peaked at 1.5 GiB and took about 1.25 s; it
  now peaks at its 341 MiB of mips and takes 0.15–0.25 s. Mip bytes match the old float chain within
  one level per channel.
