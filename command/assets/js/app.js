// Include phoenix_html to handle method=PUT/DELETE in forms and buttons
import "phoenix_html"
// Establish Phoenix Socket and LiveView configuration
import {Socket} from "phoenix"
import {LiveSocket} from "phoenix_live_view"
import topbar from "../vendor/topbar"

let csrfToken = document.querySelector("meta[name='csrf-token']").getAttribute("content")

// LiveView Hooks
let Hooks = {}

// Dark mode hook - persists preference to localStorage
Hooks.DarkMode = {
  mounted() {
    // Initialize dark mode from localStorage
    const darkMode = localStorage.getItem('darkMode') === 'true'
    this.updateTheme(darkMode)

    // Listen for toggle events from LiveView
    this.handleEvent("toggle-dark-mode", () => {
      const newValue = !document.documentElement.classList.contains('dark')
      this.updateTheme(newValue)
      localStorage.setItem('darkMode', newValue.toString())
    })
  },
  updateTheme(dark) {
    if (dark) {
      document.documentElement.classList.add('dark')
    } else {
      document.documentElement.classList.remove('dark')
    }
  }
}

// Sidebar toggle hook - handles collapse state and mobile behavior
Hooks.Sidebar = {
  mounted() {
    // Initialize sidebar state from localStorage (desktop only)
    const collapsed = localStorage.getItem('sidebarCollapsed') === 'true'
    this.pushEvent("sidebar-init", {collapsed: collapsed})

    // Handle toggle events
    this.handleEvent("toggle-sidebar", () => {
      const isCollapsed = this.el.dataset.collapsed === 'true'
      const newValue = !isCollapsed
      localStorage.setItem('sidebarCollapsed', newValue.toString())
      this.pushEvent("sidebar-toggled", {collapsed: newValue})
    })

    // Handle mobile overlay click to close
    this.handleEvent("close-mobile-sidebar", () => {
      this.pushEvent("sidebar-mobile-closed", {})
    })
  }
}

// Click outside hook for mobile sidebar
Hooks.ClickOutside = {
  mounted() {
    this.handleClickOutside = (e) => {
      if (!this.el.contains(e.target)) {
        this.pushEvent("click-outside", {})
      }
    }
    document.addEventListener("click", this.handleClickOutside)
  },
  destroyed() {
    document.removeEventListener("click", this.handleClickOutside)
  }
}

let liveSocket = new LiveSocket("/live", Socket, {
  params: {_csrf_token: csrfToken},
  hooks: Hooks
})

// Show progress bar on live navigation and form submits
topbar.config({barColors: {0: "#29d"}, shadowColor: "rgba(0, 0, 0, .3)"})
window.addEventListener("phx:page-loading-start", _info => topbar.show(300))
window.addEventListener("phx:page-loading-stop", _info => topbar.hide())

// Connect if there are any LiveViews on the page
liveSocket.connect()

// Expose liveSocket on window for web console debug logs and latency simulation:
// >> liveSocket.enableDebug()
// >> liveSocket.enableLatencySim(1000)  // enabled for duration of browser session
// >> liveSocket.disableLatencySim()
window.liveSocket = liveSocket

// Custom event handlers
window.addEventListener("phx:incident-created", (e) => {
  console.log("New incident:", e.detail)
})
