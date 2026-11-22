// SharedJSContext: Steam delivers CGameRecording_AudioSessionsChanged_Notification
// as protobuf bytes. Keep the subscription only while this page polls it.
(()=>{
 try {
  const api=window.SteamClient?.GameRecording;
  if(typeof api?.RegisterForAudioSessionsChanged!=='function') return null;
  let s=window.__deckShellAudioSessions;
  if(__RESET__ && s){ clearTimeout(s.timer); try{s.handle.unregister()}catch(_){} delete window.__deckShellAudioSessions; s=null; }
  if(!s){
   s={sessions:null, handle:null, timer:null};
   function readVarint(b,p){let n=0,m=1; for(let i=0;i<10;i++){if(p.i>=b.length)throw Error();const x=b[p.i++];n+=(x&127)*m;if(!(x&128))return n;m*=128;}throw Error()}
   function fields(b){let p={i:0},out=[];while(p.i<b.length){const tag=readVarint(b,p),n=tag>>3,w=tag&7;if(!n)throw Error();let v;if(w===0)v=readVarint(b,p);else if(w===2){const len=readVarint(b,p);if(len>b.length-p.i)throw Error();v=b.subarray(p.i,p.i+len);p.i+=len}else if(w===5){if(p.i+4>b.length)throw Error();v=b.subarray(p.i,p.i+4);p.i+=4}else if(w===1){if(p.i+8>b.length)throw Error();v=b.subarray(p.i,p.i+8);p.i+=8}else throw Error();out.push([n,w,v])}return out}
   function decode(data){const b=data instanceof Uint8Array?data:new Uint8Array(data);if(b.length>1048576)throw Error();const decoder=new TextDecoder('utf-8',{fatal:true});return fields(b).filter(x=>x[0]===1&&x[1]===2).map(x=>{let o={id:'',name:'',is_system:false,is_game:false,is_steam:false,is_captured:false,is_saved:false};for(const [n,w,v] of fields(x[2])){if(n===1&&w===2)o.id=decoder.decode(v);else if(n===2&&w===2)o.name=decoder.decode(v);else if(w===0){if(n===3)o.is_system=!!v;else if(n===6)o.is_captured=!!v;else if(n===8)o.is_game=!!v;else if(n===9)o.is_steam=!!v;else if(n===10)o.is_saved=!!v;}}return o}).filter(o=>o.id&&o.name&&!o.is_system&&!o.is_game&&!o.is_steam&&(!/spotify.exe|SpotifyWidgetProvider.exe|vlc.exe|wmplayer.exe|winamp.exe|foobar2000.exe|itunes.exe/i.test(o.id+' '+o.name)||o.is_saved)).slice(0,100)}
   s.handle=api.RegisterForAudioSessionsChanged(data=>{try{s.sessions=decode(data)}catch(_){s.sessions=null}});
   if(!s.handle || typeof s.handle.unregister!=='function') return null;
   window.__deckShellAudioSessions=s;
  }
  clearTimeout(s.timer);s.timer=setTimeout(()=>{try{s.handle.unregister()}catch(_){} if(window.__deckShellAudioSessions===s)delete window.__deckShellAudioSessions},10000);
  return s.sessions;
 } catch(_){return null}
})()
