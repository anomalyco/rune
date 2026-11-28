- **A CJK character the system's loaded CJK fonts have no longer asks the platform for a font.**
  Once a text's language had its CJK fonts loaded, every new character still went to CoreText,
  DirectWrite or fontconfig for its fallback, once each, on the UI thread: a dialog's first page
  of Japanese made about 70 lookups in one frame (half of a 22 ms frame on an M5). A character
  those fonts cover is now drawn in them without a lookup; one they lack (Simplified Chinese in a
  Japanese font) is still looked up, once.
