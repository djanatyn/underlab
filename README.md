# underlab
lab notebook and utility scripts for djanatyn's homelab

<p align="center">
  <img src="https://raw.githubusercontent.com/djanatyn/underlab/main/underlab.gif" alt="mina the hollower underlab icon gif"></img>
</p>

## inspiration

you should play [mina the hollower](https://www.yachtclubgames.com/games/mina-the-hollower/)

<p align="center">
  <img src="https://raw.githubusercontent.com/djanatyn/underlab/main/underlab.png" alt="mina the hollower underlab screenshot"></img>
</p>

## usage

```
# build an versioned artifact with the configuration planned to be applied
$ cargo run -- build pi/paperless
./build/pi-paperless-<timestamp>.tar.gz

# provision the volumes needed to run the service
$ cargo run -- provision pi/paperless

# transfer the configuration bundle and apply it remotely using cross-compilation
$ cargo run -- deploy ./build/pi-paperless-<timestamp>.tar.gz
```
