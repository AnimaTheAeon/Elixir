import asyncio, json, sys, time
from playwright.async_api import async_playwright
BASE = "http://127.0.0.1:7810/"
OUT = "/tmp/ex/shots"
import os; os.makedirs(OUT, exist_ok=True)
errors = []
async def main():
    async with async_playwright() as p:
        b = await p.chromium.launch(executable_path="/usr/bin/google-chrome", args=["--no-sandbox"])
        pg = await b.new_page(viewport={"width": 1280, "height": 800})
        pg.on("pageerror", lambda e: errors.append("pageerror: " + str(e)))
        pg.on("console", lambda m: errors.append("console: " + m.text) if m.type == "error" else None)
        await pg.goto(BASE)
        await pg.wait_for_timeout(600)
        await pg.screenshot(path=f"{OUT}/00-splash.png")
        await pg.wait_for_selector("#splash", state="detached", timeout=12000)
        # Rename: choose the torrents folder and scan it.
        await pg.click("#choose")
        await pg.click(".picker-item[data-path$='/data']")
        await pg.click(".picker-item[data-path$='/torrents']")
        await pg.click("#picker-choose")
        await pg.wait_for_function("document.querySelectorAll('#rename-list .item').length >= 5 && !document.querySelector('#rename-list .st-matching')", timeout=20000)
        await pg.screenshot(path=f"{OUT}/01-rename-ff.png")
        label = await pg.inner_text("#rename-go")
        print("button:", label)
        # Pick another match on the Bear.
        await pg.click("#rename-list .item:has-text('The.Bear') [data-pick]")
        await pg.wait_for_selector("#matcher.open")
        await pg.fill("#m-query", "Shogun")
        await pg.click("#m-search")
        await pg.wait_for_selector("#m-list [data-cand]")
        n = await pg.locator("#m-list [data-cand]").count()
        print("shogun results:", n)
        await pg.screenshot(path=f"{OUT}/02-matcher.png")
        await pg.click("#m-cancel")
        # Rename all selected.
        await pg.click("#rename-go")
        await pg.wait_for_function("document.querySelectorAll('#rename-list .st-done').length >= 4", timeout=30000)
        await pg.screenshot(path=f"{OUT}/03-renamed.png")
        # History, undo.
        await pg.click(".menu-item[data-go=history]")
        await pg.wait_for_selector("#history-list .run")
        await pg.screenshot(path=f"{OUT}/04-history-ff.png")
        pg.once("dialog", lambda d: asyncio.ensure_future(d.accept()))
        await pg.click("#history-list [data-undo]")
        await pg.wait_for_selector("#history-list .tag:has-text('UNDONE')", timeout=10000)
        print("undo ok")
        # Automation: add folder, run checks, toggle on.
        await pg.click(".menu-item[data-go=auto]")
        await pg.click("#add-watched")
        await pg.click(".picker-item[data-path$='/data']")
        await pg.click(".picker-item[data-path$='/torrents']")
        await pg.click("#picker-choose")
        await pg.wait_for_selector("#watched .party")
        await pg.click("#preflight-run")
        await pg.wait_for_selector("#preflight .pf")
        await pg.click("#auto-toggle")
        await pg.wait_for_timeout(2500)
        await pg.screenshot(path=f"{OUT}/05-auto-ff.png")
        print("auto line:", await pg.inner_text("#auto-line"))
        # Queue shows the "already there" offer.
        await pg.click(".menu-item[data-go=queue]")
        await pg.wait_for_selector("#old-offers .offer")
        await pg.screenshot(path=f"{OUT}/06-queue-ff.png")
        # Settings test buttons.
        await pg.click(".menu-item[data-go=config]")
        for svc in ["lifestream", "midgar", "turks", "tmdb", "tvdb"]:
            await pg.click(f"[data-test={svc}]")
            await pg.wait_for_function(f"!document.querySelector('[data-out={svc}]').textContent.includes('Testing')", timeout=15000)
            print(svc, "->", await pg.inner_text(f"[data-out={svc}]"))
        await pg.screenshot(path=f"{OUT}/07-settings-ff.png", full_page=False)
        # Every look: each page.
        for theme in ["green", "blades", "glass", "ff"]:
            await pg.click(".menu-item[data-go=config]") if theme != "green" and await pg.locator(".menu-item[data-go=config]").is_visible() else None
            await pg.evaluate("(t) => { config.theme = t; applyTheme(t); queueSave({theme: t}, true); }", theme)
            await pg.wait_for_timeout(900)
            for pid in ["queue", "rename", "auto", "history", "config"]:
                await pg.evaluate("(id) => { if (isGreen()) { greenHome = false; document.body.classList.remove('g-at-home'); } go(id); }", pid)
                await pg.wait_for_timeout(900)
                await pg.screenshot(path=f"{OUT}/t-{theme}-{pid}.png")
            if theme == "green":
                await pg.evaluate("greenShowHome(false)")
                await pg.wait_for_timeout(400)
                await pg.screenshot(path=f"{OUT}/t-green-home.png")
        await b.close()
asyncio.run(main())
print("ERRORS:", errors or "none")
