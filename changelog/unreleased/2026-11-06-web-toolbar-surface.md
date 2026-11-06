- **A web page in document mode no longer reconfigures its surface as a phone's toolbar moves.**
  The document canvas was `height: 100%`, so it resized with Safari's URL bar collapsing and
  expanding during a scroll; every step configured the surface again, which destroys and
  reallocates full-screen swapchain textures in the GPU process mid-gesture. The canvas is now
  as tall as the largest viewport (`100lvh`), and a document's surface never shrinks for a
  toolbar-sized height change, so a collapse and expand costs no configuration at all. A phone
  also stops committing a frame for each of the visual viewport events its toolbar sends, unless
  the safe area or the largest viewport actually changed.
