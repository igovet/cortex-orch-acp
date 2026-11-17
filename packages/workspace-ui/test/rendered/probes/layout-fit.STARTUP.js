const logo = document.querySelector('.l-startup-logo img'), slot = document.querySelector('.l-startup-slot'), box = logo && logo.getBoundingClientRect();
const frame = [...document.querySelectorAll('.titlebar, .sidebar, .ws-tabstrip, .main, .card')].map(e => e.className);
const problems = [];
if (!logo || !logo.complete || !logo.naturalWidth) problems.push('startup logo missing');
if (box && Math.abs(box.left + box.width / 2 - innerWidth / 2) > .5 || box && Math.abs(box.top + box.height / 2 - innerHeight / 2) > .5) problems.push(`startup logo off centre: ${box.left},${box.top}`);
if (frame.length) problems.push('partial interface during startup: ' + frame.join(', '));
const win = [...document.querySelectorAll('.l-startup-band .win button')].map(b => b.getAttribute('aria-label'));
if (win.length !== 3) problems.push('window controls missing: ' + win.join(', '));
return {problems, logo: box && [box.left, box.top, box.width, box.height], slot: slot ? slot.textContent.trim() : null, win};
