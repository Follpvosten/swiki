# s(imple )wiki
This is a proof-of-concept minimal wiki software built in Rust in about one work week.
Mostly intended as a playfield to try out Rocket and sled, but also with a clear
goal and that is being fast and minimal in complexity, while offering basic wiki
functionality (like MediaWiki, Wiki.js).

Another goal is to also have a simple API so I can easily integrate it with
other platforms through chatbots and similar.

I haven't set up any badges yet, but I have near 90% code coverage at the time
of writing this README (because whenever I was bored and had nothing else to do,
I just went and wrote a bunch of tests).

I will do a "release" once sled 1.0 hits.

## Features ToDo
* [x] Basic password-based registration/login system
  * [x] Simple captchas
* [ ] Articles
  * [x] Creation
  * [x] Editing
  * [x] Renaming
  * [x] Markdown rendering
  * [x] Revision history
  * [ ] Deletion
* [x] Search system (I'm using Tantivy)
* [ ] Admin settings
  * [x] Disabling registration
  * [ ] Possibly other management stuff
* [ ] API

A ToDo on the Horizon is also updating the UI; currently I'm using Bulma without
any changes, I'd probably like to exclude parts I'm not using and also some custom
theming would be nice.
