// Scope fragment for «Отправить на телефон» (tasks 313, 315). The MobileDevice module is found by text marker, and its
// factory is re-run once in a private scope with an extra export line. Only __phoneCheck is a network call, and it is a
// query (HasMobileDevice). The export and upload helpers below are used only by the live send in phone_send.js.
const __req=(()=>{let r=null;window.webpackChunksteamui.push([["fable-send-phone-"+Math.random()],{},(x)=>{r=x}]);return r})();
const __findMod=(needle)=>{const ids=Object.keys(__req.m).filter(id=>__req.m[id].toString().includes(needle));if(ids.length!==1)throw new Error("module marker not unique: "+needle+" ("+ids.length+")");return ids[0];};
const __idMobile=__findMod("MobileDevice.HasMobileDevice#1");
const __src=__req.m[__idMobile].toString();
if(!/^\(\w+,_,\w+\)=>\{/.test(__src)||!/\}\s*$/.test(__src))throw new Error("MobileDevice module shape changed");
const __exp={};
(0,eval)("("+__src.replace(/\}\s*$/,";_.__scope={P:typeof P!=='undefined'?P:null,hi:typeof hi!=='undefined'?hi:null,Req:typeof x!=='undefined'?x:null,AppType:typeof m!=='undefined'?m:null,Enums:typeof u!=='undefined'?u:null,Res:typeof r!=='undefined'?r:null,Lib:typeof l!=='undefined'?l:null};}")+")")({exports:__exp},__exp,__req);
const __S=__exp.__scope;
if(!__S||typeof __S.P?.HasMobileDevice!=="function"||typeof __S.Req!=="function"||typeof __S.hi!=="function"||!__S.Lib?.w||!__S.Res||!__S.Enums)throw new Error("MobileDevice scope incomplete");
// GameID class (module with the export VS). Used only for content_game_id on upload.
const __GameID=(()=>{const ids=Object.keys(__req.m).filter(id=>/\bVS:\(\)=>/.test(__req.m[id].toString()));if(ids.length!==1)throw new Error("GameID module not unique ("+ids.length+")");return __req(ids[0]).VS;})();
const __phoneCheck=async(min)=>{
  const w=__S.Lib.w.Init(__S.Req);
  w.Body().set_push_enabled_only(true);
  w.Body().set_app_type(__S.AppType);
  w.Body().set_minimum_version(min);
  const res=await __S.P.HasMobileDevice(window.cm.GetServiceTransport(),w);
  if(res.GetEResult()!==__S.Res.R)return {query_ok:false,eresult:res.GetEResult()};
  return {query_ok:true,eresult:res.GetEResult(),found_device:!!res.Body().found_device(),up_to_date:!!res.Body().up_to_date()};
};
// File from a local path, as stock u() in module 96513 does it: SetPendingFilePath, then a hidden file input whose
// accept token resolves to that path. Nothing is read or changed on disk.
const __pickFile=async(path)=>{
  const tok="fable"+Date.now().toString(36)+Math.random().toString(36).slice(2,8);
  if(!(await SteamClient.Browser.SetPendingFilePath(tok,path)))throw new Error("Unable to get File object");
  return await new Promise((resolve,reject)=>{
    const doc=window.document;const x=doc.createElement("input");
    x.setAttribute("type","file");x.setAttribute("accept",".valvefile"+tok);x.style.display="none";
    const done=()=>{try{doc.body.removeChild(x)}catch(_){}};
    const t=setTimeout(()=>{done();reject(new Error("file pick timed out"))},15000);
    x.addEventListener("change",()=>{clearTimeout(t);const f=x.files[0];done();f?resolve(f):reject(new Error("no file"))});
    x.addEventListener("cancel",()=>{clearTimeout(t);done();reject(new Error("Unable to get File object"))});
    doc.body.appendChild(x);SteamClient.Browser.NotifyUserActivation();x.click();
  });
};
