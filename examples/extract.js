// `ema2 js --file examples/extract.js`
// 在页面主世界执行；可以直接 return（语法错误时自动回退为 async 函数体）。
const title = document.title;
const headings = [...document.querySelectorAll('h1,h2,h3')]
  .map(el => el.innerText.replace(/\s+/g, ' ').trim())
  .filter(Boolean)
  .slice(0, 20);
const links = [...document.querySelectorAll('a[href]')]
  .slice(0, 20)
  .map(a => ({ text: a.innerText.trim().slice(0, 60), href: a.href }));

return { title, headings, links };
