const list = document.querySelector('#streams')
const message = document.querySelector('#message')
const template = document.querySelector('#stream-template')

async function api(url, options = {}) {
  const response = await fetch(url, options)
  if (response.status === 401) {
    location.href = '/admin/login'
    throw new Error('Unauthorized')
  }
  const data = await response.json().catch(() => ({}))
  if (!response.ok) throw new Error(data.error || `Request failed (${response.status})`)
  return data
}

const jsonOptions = (method, body) => ({
  method,
  headers: {'Content-Type': 'application/json'},
  body: JSON.stringify(body),
})

function showSecret(card, label, value) {
  const box = card.querySelector('.secret')
  box.hidden = false
  box.querySelector('.secret-label').textContent = label
  box.querySelector('.secret-value').textContent = value
  box.querySelector('.copy').onclick = async () => {
    await navigator.clipboard.writeText(value)
    box.querySelector('.copy').textContent = 'Copied'
  }
}

async function loadViewerKeys(stream, container) {
  const data = await api(`/api/live/streams/${stream.id}/viewer-keys`)
  container.replaceChildren()
  if (!data.viewer_keys.length) {
    container.textContent = 'No viewer keys yet.'
    return
  }
  data.viewer_keys.forEach(key => {
    const row = document.createElement('div')
    row.className = 'viewer-key'
    const text = document.createElement('div')
    const status = key.revoked_at ? 'Revoked' : `Expires ${new Date(`${key.expires_at}Z`).toLocaleString()}`
    text.innerHTML = `<strong></strong><small></small>`
    text.querySelector('strong').textContent = key.label
    text.querySelector('small').textContent = status
    const revoke = document.createElement('button')
    revoke.textContent = 'Revoke'
    revoke.disabled = Boolean(key.revoked_at)
    revoke.onclick = async () => {
      await api(`/api/live/viewer-keys/${key.id}/revoke`, {method: 'POST'})
      await loadViewerKeys(stream, container)
    }
    row.append(text, revoke)
    container.append(row)
  })
}

function renderStream(stream, initialSecret) {
  const card = template.content.firstElementChild.cloneNode(true)
  card.querySelector('.stream-title').textContent = stream.title
  card.querySelector('.public-id').textContent = stream.public_id
  card.querySelector('.obs-server').textContent = `${location.protocol === 'https:' ? 'rtmp' : 'rtmp'}://${location.hostname}/live`
  const status = card.querySelector('.status')
  status.textContent = stream.is_live ? 'Live now' : 'Offline'
  status.classList.toggle('live', stream.is_live)
  const enabled = card.querySelector('.enabled')
  enabled.checked = stream.enabled
  enabled.onchange = async () => {
    try { await api(`/api/live/streams/${stream.id}/enabled`, jsonOptions('PATCH', {enabled: enabled.checked})) }
    catch (error) { enabled.checked = !enabled.checked; message.textContent = error.message }
  }
  card.querySelector('.rotate').onclick = async () => {
    if (!confirm('The current OBS key will stop working. Continue?')) return
    const data = await api(`/api/live/streams/${stream.id}/publish-key`, {method: 'POST'})
    showSecret(card, 'New OBS stream key', data.obs_stream_key)
  }
  card.querySelector('.viewer').onclick = async () => {
    const label = prompt('Who is this link for?')
    if (!label) return
    const rawHours = prompt('How many hours should it work?', '168')
    if (!rawHours) return
    const data = await api(`/api/live/streams/${stream.id}/viewer-keys`, jsonOptions('POST', {
      label,
      expires_hours: Number(rawHours),
    }))
    showSecret(card, 'New viewer link', `${location.origin}${data.watch_path}`)
    await loadViewerKeys(stream, card.querySelector('.viewer-keys'))
  }
  card.querySelector('details').ontoggle = event => {
    if (event.target.open) loadViewerKeys(stream, card.querySelector('.viewer-keys')).catch(error => message.textContent = error.message)
  }
  if (initialSecret) showSecret(card, 'OBS stream key', initialSecret)
  list.prepend(card)
}

async function load() {
  const data = await api('/api/live/streams')
  list.replaceChildren()
  data.streams.reverse().forEach(stream => renderStream(stream))
}

document.querySelector('#create-form').onsubmit = async event => {
  event.preventDefault()
  message.textContent = ''
  try {
    const data = await api('/api/live/streams', jsonOptions('POST', {title: document.querySelector('#title').value}))
    renderStream(data.stream, data.obs_stream_key)
    event.target.reset()
  } catch (error) { message.textContent = error.message }
}
document.querySelector('#logout').onclick = async () => {
  await fetch('/admin/logout', {method: 'POST'})
  location.href = '/admin/login'
}
load().catch(error => message.textContent = error.message)
