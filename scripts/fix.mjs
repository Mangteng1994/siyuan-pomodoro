const U='http://127.0.0.1:6807', H={'Content-Type':'application/json'};
const post=async(p,b)=>(await fetch(U+p,{method:'POST',headers:H,body:JSON.stringify(b)})).json();
const NB='20261102214656-ofh8oh0';
console.log(await post('/api/notebook/renameNotebook',{notebook:NB,name:'测试笔记本'}));
console.log(await post('/api/filetree/renameDocByID',{id:'20261102214709-yrv0cok',title:'测试文档'}));
console.log(await post('/api/block/updateBlock',{id:'20261102214709-in6q62v',dataType:'markdown',data:'这是用来测试番茄钟的段落'}));
console.log(JSON.stringify(await post('/api/block/getChildBlocks',{id:'20261102214709-yrv0cok'})));
