export interface RealtimeEnvelope<T = unknown> {
  id?: string
  event: string
  channel?: string
  payload: T
}

export type RealtimeStatus = 'idle' | 'connecting' | 'connected' | 'reconnecting' | 'closed'

export interface RealtimeClientOptions {
  url?: string
  reconnect?: boolean
  reconnectDelay?: number
  maxReconnectDelay?: number
  heartbeatInterval?: number
  heartbeatTimeout?: number
}

type Listener = (message: RealtimeEnvelope) => void
type StatusListener = (status: RealtimeStatus) => void

export class RealtimeClient {
  private readonly url: string
  private readonly reconnect: boolean
  private readonly reconnectDelay: number
  private readonly maxReconnectDelay: number
  private readonly heartbeatInterval: number
  private readonly heartbeatTimeout: number
  private socket: WebSocket | null = null
  private readonly channels = new Set<string>()
  private readonly listeners = new Map<string, Set<Listener>>()
  private readonly statusListeners = new Set<StatusListener>()
  private retryCount = 0
  private explicitlyClosed = false
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null
  private heartbeatTimer: ReturnType<typeof setInterval> | null = null
  private heartbeatDeadline: ReturnType<typeof setTimeout> | null = null
  private _status: RealtimeStatus = 'idle'

  constructor(options: RealtimeClientOptions = {}) {
    this.url = options.url ?? RealtimeClient.defaultUrl()
    this.reconnect = options.reconnect ?? true
    this.reconnectDelay = options.reconnectDelay ?? 1_000
    this.maxReconnectDelay = options.maxReconnectDelay ?? 15_000
    this.heartbeatInterval = options.heartbeatInterval ?? 25_000
    this.heartbeatTimeout = options.heartbeatTimeout ?? 10_000
  }

  get status() {
    return this._status
  }

  static defaultUrl() {
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
    return `${protocol}//${window.location.host}/ws`
  }

  connect() {
    if (
      this.socket?.readyState === WebSocket.OPEN ||
      this.socket?.readyState === WebSocket.CONNECTING
    ) return

    this.explicitlyClosed = false
    this.setStatus(this.retryCount > 0 ? 'reconnecting' : 'connecting')
    const socket = new WebSocket(this.url)
    this.socket = socket

    socket.addEventListener('message', (event) => {
      let message: RealtimeEnvelope
      try {
        message = JSON.parse(String(event.data)) as RealtimeEnvelope
      } catch {
        return
      }

      if (!message || typeof message.event !== 'string') return

      if (message.event === 'connection.established') {
        this.retryCount = 0
        this.setStatus('connected')
        this.startHeartbeat()
        for (const channel of this.channels) {
          this.send({ action: 'subscribe', channel })
        }
      } else if (message.event === 'pong') {
        this.clearHeartbeatDeadline()
      }

      this.emit('message', message)
      this.emit(message.event, message)
      if (message.channel) this.emit(`${message.channel}:${message.event}`, message)
    })

    socket.addEventListener('close', () => {
      if (this.socket !== socket) return
      this.socket = null
      this.stopHeartbeat()
      this.setStatus(this.explicitlyClosed ? 'closed' : 'reconnecting')
      if (!this.explicitlyClosed && this.reconnect) this.scheduleReconnect()
    })

    socket.addEventListener('error', () => {
      // The close event owns reconnection. Closing here also handles browsers
      // that leave a failed handshake in CONNECTING for too long.
      socket.close()
    })
  }

  disconnect() {
    this.explicitlyClosed = true
    this.clearReconnectTimer()
    this.stopHeartbeat()
    this.socket?.close()
    this.socket = null
    this.setStatus('closed')
  }

  subscribe(channel: string) {
    this.channels.add(channel)
    this.connect()
    this.send({ action: 'subscribe', channel })
    return () => this.unsubscribe(channel)
  }

  unsubscribe(channel: string) {
    this.channels.delete(channel)
    this.send({ action: 'unsubscribe', channel })
  }

  on<T = unknown>(event: string, listener: (message: RealtimeEnvelope<T>) => void) {
    const untypedListener = listener as Listener
    if (!this.listeners.has(event)) this.listeners.set(event, new Set())
    this.listeners.get(event)?.add(untypedListener)
    return () => this.listeners.get(event)?.delete(untypedListener)
  }

  onStatus(listener: StatusListener) {
    this.statusListeners.add(listener)
    listener(this._status)
    return () => this.statusListeners.delete(listener)
  }

  private send(command: object) {
    if (this.socket?.readyState === WebSocket.OPEN) {
      this.socket.send(JSON.stringify(command))
    }
  }

  private emit(event: string, message: RealtimeEnvelope) {
    for (const listener of this.listeners.get(event) ?? []) {
      try {
        listener(message)
      } catch (error) {
        console.error(`Realtime listener failed for ${event}`, error)
      }
    }
  }

  private setStatus(status: RealtimeStatus) {
    if (status === this._status) return
    this._status = status
    for (const listener of this.statusListeners) listener(status)
  }

  private startHeartbeat() {
    this.stopHeartbeat()
    this.heartbeatTimer = setInterval(() => {
      if (this.socket?.readyState !== WebSocket.OPEN) return
      const id = globalThis.crypto?.randomUUID?.() ?? `${Date.now()}`
      this.send({ action: 'ping', id })
      this.clearHeartbeatDeadline()
      this.heartbeatDeadline = setTimeout(() => this.socket?.close(), this.heartbeatTimeout)
    }, this.heartbeatInterval)
  }

  private stopHeartbeat() {
    if (this.heartbeatTimer) clearInterval(this.heartbeatTimer)
    this.heartbeatTimer = null
    this.clearHeartbeatDeadline()
  }

  private clearHeartbeatDeadline() {
    if (this.heartbeatDeadline) clearTimeout(this.heartbeatDeadline)
    this.heartbeatDeadline = null
  }

  private scheduleReconnect() {
    this.clearReconnectTimer()
    const baseDelay = Math.min(
      this.reconnectDelay * 2 ** this.retryCount,
      this.maxReconnectDelay,
    )
    const delay = baseDelay * (0.8 + Math.random() * 0.4)
    this.retryCount += 1
    this.reconnectTimer = setTimeout(() => this.connect(), delay)
  }

  private clearReconnectTimer() {
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer)
    this.reconnectTimer = null
  }
}
