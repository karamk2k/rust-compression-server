import videojs from 'video.js'
import 'video.js/dist/video-js.css'
import './live.css'

const element = document.querySelector<HTMLVideoElement>('#live-player')
const status = document.querySelector<HTMLElement>('#player-status')

if (element) {
  const streamId = element.dataset.streamId
  const player = videojs(element, {
    autoplay: false,
    controls: true,
    fluid: true,
    liveui: true,
    responsive: true,
    html5: {vhs: {withCredentials: true}},
    sources: [{src: `/hls/${streamId}.m3u8`, type: 'application/x-mpegURL'}],
  })
  player.on('playing', () => { if (status) status.textContent = 'Live' })
  player.on('waiting', () => { if (status) status.textContent = 'Waiting for video…' })
  player.on('error', () => {
    if (status) status.textContent = 'The stream is offline or temporarily unavailable.'
  })
}
