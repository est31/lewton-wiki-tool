# lewton wiki tool

This is a tool written in Rust to:

* Download ogg/vorbis media from Wikimedia commons
* Decode it with libvorbis as well as lewton
* Compare the output

The [end goal](https://github.com/RustAudio/lewton/issues/36) is to prove that lewton produces similar values to libvorbis
for almost all samples in Wikimedia commons.

## License

Licensed under Apache 2 or MIT (at your option). For details, see the [LICENSE](LICENSE) file.

## TODO

* List other info in the json like encoder string, duration, channel count
* Don't request everything in parallel at once but have a number of parallel requests param
