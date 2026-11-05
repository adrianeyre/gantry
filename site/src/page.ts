// The page's enhancements. It works without them: each control this adds is
// `hidden` in the HTML until it is shown here.

/** The menu button, on narrow screens. */
function menu() {
  const button = document.querySelector<HTMLButtonElement>('.menu-button')
  const nav = document.getElementById('site-nav')
  if (!button || !nav) return
  button.hidden = false
  const close = () => {
    nav.classList.remove('open')
    button.setAttribute('aria-expanded', 'false')
  }
  button.addEventListener('click', () => {
    const open = nav.classList.toggle('open')
    button.setAttribute('aria-expanded', String(open))
  })
  nav.addEventListener('click', (event) => {
    if ((event.target as Element).closest('a')) close()
  })
  document.addEventListener('keydown', (event) => {
    if (event.key === 'Escape' && nav.classList.contains('open')) {
      close()
      button.focus()
    }
  })
}

/** The header gains a rule once the page has scrolled under it. */
function header() {
  const bar = document.querySelector('.site-header')
  const top = document.getElementById('top')
  if (!bar || !top) return
  new IntersectionObserver(([entry]) => bar.classList.toggle('scrolled', !entry.isIntersecting), {
    rootMargin: '-1px 0px 0px 0px',
  }).observe(top.querySelector('.hero-copy') ?? top)
}

/** Copy buttons: `data-copy` holds the text, or the code block after the button is copied. */
function copyButtons() {
  if (!navigator.clipboard) return
  for (const button of document.querySelectorAll<HTMLButtonElement>('.copy')) {
    button.hidden = false
    const label = button.querySelector('.visually-hidden')
    const original = label?.textContent ?? ''
    button.addEventListener('click', async () => {
      const text = button.dataset.copy ?? button.nextElementSibling?.textContent ?? ''
      try {
        await navigator.clipboard.writeText(text.trim())
        button.classList.add('copied')
        if (label) label.textContent = 'Copied'
        setTimeout(() => {
          button.classList.remove('copied')
          if (label) label.textContent = original
        }, 2000)
      } catch {
        // Clipboard refused (an insecure context, say): the text is selectable.
      }
    })
  }
}

/** Tabs (the ways to run Gantry), with the arrow keys moving between them. */
function tabs() {
  for (const tabs of document.querySelectorAll<HTMLElement>('[data-tabs]')) {
    const list = tabs.querySelector<HTMLElement>('[role="tablist"]')
    if (!list) continue
    const buttons = [...tabs.querySelectorAll<HTMLButtonElement>('[role="tab"]')]
    const select = (chosen: HTMLButtonElement, focus: boolean) => {
      for (const button of buttons) {
        const selected = button === chosen
        button.setAttribute('aria-selected', String(selected))
        button.tabIndex = selected ? 0 : -1
        const panel = document.getElementById(button.getAttribute('aria-controls') ?? '')
        if (panel) panel.hidden = !selected
      }
      if (focus) chosen.focus()
    }
    list.hidden = false
    tabs.classList.add('ready')
    for (const panel of tabs.querySelectorAll<HTMLElement>('[role="tabpanel"]')) panel.tabIndex = 0
    select(buttons[0], false)
    list.addEventListener('click', (event) => {
      const button = (event.target as Element).closest<HTMLButtonElement>('[role="tab"]')
      if (button) select(button, false)
    })
    list.addEventListener('keydown', (event) => {
      const index = buttons.indexOf(document.activeElement as HTMLButtonElement)
      if (index < 0) return
      const moves: Record<string, number> = { ArrowRight: 1, ArrowLeft: -1, Home: -index, End: buttons.length - 1 - index }
      if (!(event.key in moves)) return
      event.preventDefault()
      select(buttons[(index + moves[event.key] + buttons.length) % buttons.length], true)
    })
  }
}

/** The screenshot viewer: a link to the full-size picture, opened in a modal dialog instead. */
function viewer() {
  const dialog = document.querySelector<HTMLDialogElement>('dialog.viewer')
  if (!dialog || typeof dialog.showModal !== 'function') return
  const image = dialog.querySelector('img')!
  const caption = dialog.querySelector('.viewer-caption')!
  let opener: HTMLElement | null = null
  for (const link of document.querySelectorAll<HTMLAnchorElement>('[data-viewer]')) {
    link.addEventListener('click', (event) => {
      event.preventDefault()
      image.src = link.href
      image.alt = link.querySelector('img')?.alt ?? ''
      caption.textContent = link.closest('figure')?.querySelector('figcaption')?.textContent ?? ''
      opener = link
      dialog.showModal()
    })
  }
  dialog.addEventListener('click', (event) => {
    if (event.target === dialog) dialog.close()
  })
  dialog.addEventListener('close', () => opener?.focus())
}

/** "Download for <your system>": point the hero's button at this system's card, and mark the card. */
function download() {
  const nav = navigator as Navigator & { userAgentData?: { platform?: string } }
  const platform = (nav.userAgentData?.platform ?? navigator.platform ?? '').toLowerCase()
  const agent = navigator.userAgent.toLowerCase()
  const os = /win/.test(platform)
    ? 'windows'
    : /mac/.test(platform) && !/iphone|ipad/.test(agent)
      ? 'mac'
      : /linux/.test(platform) && !/android/.test(agent)
        ? 'linux'
        : null
  const names = { windows: 'Windows', mac: 'macOS', linux: 'Linux' }
  const card = os && document.querySelector(`.download[data-os="${os}"]`)
  const button = document.querySelector<HTMLAnchorElement>('[data-os-download]')
  if (!os || !card || !button) return
  card.classList.add('suggested')
  const primary = card.querySelector<HTMLAnchorElement>('.button-primary')
  if (primary) button.href = primary.href
  const label = button.querySelector('[data-os-label]')
  if (label) label.textContent = `Download for ${names[os]}`
}

export function enhance() {
  menu()
  header()
  copyButtons()
  tabs()
  viewer()
  download()
}
