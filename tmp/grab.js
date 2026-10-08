(()=>{
  const t=document.querySelector('#class-bxqkb table');
  if(!t) return 'NO TABLE';
  const grid=[]; // grid[r][c] = text
  const rows=Array.from(t.rows);
  rows.forEach((tr,ri)=>{
    if(!grid[ri]) grid[ri]=[];
    let ci=0;
    Array.from(tr.cells).forEach(td=>{
      while(grid[ri][ci]!==undefined) ci++;
      const txt=(td.textContent||'').replace(/\s+/g,' ').trim();
      const span=td.rowSpan||1;
      for(let k=0;k<span;k++){
        if(!grid[ri+k]) grid[ri+k]=[];
        grid[ri+k][ci]=txt;
      }
      ci+=1;
    });
  });
  return grid.map(r=>r.map(x=>x===undefined?'':x).join(' || ')).join('\n');
})()
