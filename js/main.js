function switchTab(name) {
  document.querySelectorAll('.install-tab').forEach(function(b) { b.classList.remove('active'); });
  document.querySelectorAll('.install-pane').forEach(function(p) { p.classList.remove('active'); });
  document.querySelector('[onclick="switchTab(\'' + name + '\')"]').classList.add('active');
  document.getElementById('pane-' + name).classList.add('active');
}

function copyCmd(btn, text) {
  navigator.clipboard.writeText(text).then(function() {
    btn.innerHTML = '&#x2713;';
    btn.classList.add('copied');
    setTimeout(function() { btn.innerHTML = '&#x29C9;'; btn.classList.remove('copied'); }, 2000);
  });
}

(function() {
  var slides = document.getElementById('demo-slides');
  var dots = document.getElementById('demo-dots');
  var count = slides.children.length;
  var idx = 0;
  var timer;

  for (var i = 0; i < count; i++) {
    (function(i) {
      var d = document.createElement('button');
      d.className = 'demo-dot';
      if (i === 0) d.classList.add('active');
      d.onclick = function() { go(i); };
      dots.appendChild(d);
    })(i);
  }

  function go(i) {
    idx = i;
    slides.style.transform = 'translateX(-' + (idx * 100) + '%)';
    document.querySelectorAll('.demo-dot').forEach(function(d, j) {
      d.classList.toggle('active', j === idx);
    });
    resetTimer();
  }

  function next() { go((idx + 1) % count); }
  function prev() { go((idx - 1 + count) % count); }
  function resetTimer() { clearInterval(timer); timer = setInterval(next, 5000); }

  window.demoNext = next;
  window.demoPrev = prev;
  window.demoGo = go;
  resetTimer();
})();

var billingYearly = false;
function toggleBilling() {
  billingYearly = !billingYearly;
  var sw = document.querySelector('.toggle-switch');
  var monthly = document.getElementById('billing-monthly');
  var yearly = document.getElementById('billing-yearly');
  if (billingYearly) {
    sw.classList.add('yearly');
    monthly.classList.remove('active');
    yearly.classList.add('active');
    document.querySelectorAll('.period').forEach(function(p) { p.innerHTML = '/yr <span class="discount">(&minus;20%)</span>'; });
  } else {
    sw.classList.remove('yearly');
    monthly.classList.add('active');
    yearly.classList.remove('active');
    document.querySelectorAll('.period').forEach(function(p) { p.textContent = '/mo'; });
  }
}

(function() {
  var links = document.querySelectorAll('.toc a');
  if (!links.length) return;
  var sections = [];
  links.forEach(function(a) {
    var id = a.getAttribute('href').replace('#', '');
    var el = document.getElementById(id);
    if (el) sections.push({link: a, top: el});
  });
  function update() {
    var scrollY = window.scrollY + 100;
    var active = null;
    sections.forEach(function(s) {
      if (s.top.offsetTop <= scrollY) active = s.link;
    });
    links.forEach(function(l) { l.classList.remove('active'); });
    if (active) active.classList.add('active');
  }
  window.addEventListener('scroll', update, {passive: true});
  update();
})();
